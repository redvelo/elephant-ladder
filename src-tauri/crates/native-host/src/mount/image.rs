//! Core media type images the engine cannot decode, restated as PNG.
//!
//! EPUB 3.4 made AVIF and JPEG XL core media types, so a conforming publication may
//! reference them with no fallback of any kind. Engines disagree about both: desktop
//! CEF decodes AVIF but not JPEG XL, and the Android System WebView decodes neither.
//!
//! The engine states which image formats it decodes on every image request, so the
//! decision needs no capability table and no platform test: a format the request does
//! not name is decoded here and re-encoded as PNG, which every engine decodes. PNG is
//! lossless, so the restated image carries the authored pixels and any alpha channel,
//! and a browser that later gains the format stops paying for the conversion on its
//! own.
//!
//! An image past its bound, one whose decode fails, or a format the request already
//! names is left to the streaming path exactly as authored.

use crate::{ActiveReaderSession, MAX_ADAPTED_IMAGE_BYTES, adapted_image_decode_budget};
use axum::body::Bytes;
use axum::http::{HeaderMap, header::ACCEPT};
use epub_stack::EpubPath;
use jxl_oxide::{EnumColourEncoding, JxlImage, RenderingIntent};
use png::{BitDepth, ColorType, Encoder};
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// A restated image and the size it decoded to, which is what its cost scales with.
struct RestatedImage {
    png: Vec<u8>,
    width: u32,
    height: u32,
    /// Decoding the authored format, separately from re-encoding it, because only one of
    /// the two is a choice this host makes.
    decode_ms: f64,
}

/// A core media type image this host can decode when the engine cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdaptableImage {
    Avif,
    Jxl,
}

impl AdaptableImage {
    pub(crate) fn media_type(self) -> &'static str {
        match self {
            Self::Avif => "image/avif",
            Self::Jxl => "image/jxl",
        }
    }
}

/// Whether the manifest declares this resource an image this host could restate.
pub(crate) fn adaptable_image(mime: &str) -> Option<AdaptableImage> {
    match mime {
        "image/avif" => Some(AdaptableImage::Avif),
        "image/jxl" => Some(AdaptableImage::Jxl),
        _ => None,
    }
}

/// Whether the request names this media type as one the engine decodes.
///
/// Chromium lists every image format it has a decoder for and then always adds
/// `image/*` and `*/*`, so a wildcard says nothing about the decoder and only an exact
/// member is evidence. A request that carries no `Accept` at all is not evidence
/// either, and is treated as naming nothing.
pub(crate) fn request_names_image_type(headers: &HeaderMap, media_type: &str) -> bool {
    headers
        .get_all(ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|entry| {
            let candidate = entry.split(';').next().unwrap_or_default().trim();
            candidate.eq_ignore_ascii_case(media_type)
        })
}

/// The conversion this request needs, or `None` when the engine decodes the authored
/// format itself.
pub(crate) fn image_adaptation(headers: &HeaderMap, mime: &str) -> Option<AdaptableImage> {
    let kind = adaptable_image(mime)?;
    (!request_names_image_type(headers, kind.media_type())).then_some(kind)
}

pub(crate) async fn image_representation(
    session: Arc<ActiveReaderSession>,
    path: EpubPath,
    size_bytes: u64,
    kind: AdaptableImage,
) -> Option<Bytes> {
    if size_bytes > MAX_ADAPTED_IMAGE_BYTES {
        return None;
    }
    tokio::task::spawn_blocking(move || {
        if session.revoked.load(Ordering::Acquire) {
            return None;
        }
        let bytes = session
            .entry
            .epub
            .read_with(&path, |reader| {
                let mut bytes = Vec::new();
                reader
                    .take(MAX_ADAPTED_IMAGE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes)
            })
            .ok()?
            .ok()?;
        if bytes.len() as u64 > MAX_ADAPTED_IMAGE_BYTES {
            return None;
        }
        let started = std::time::Instant::now();
        let png = match kind {
            AdaptableImage::Avif => avif_as_png(&bytes),
            AdaptableImage::Jxl => jxl_as_png(&bytes),
        };
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        match png {
            Ok(restated) => {
                // What this costs on the reading device is the reason the conversion is
                // bounded, so every one is measured where a collector can read it.
                crate::session::log_performance(serde_json::json!({
                    "event": "image_restated",
                    "path": path.as_str(),
                    "format": kind.media_type(),
                    "sourceBytes": bytes.len(),
                    "pngBytes": restated.png.len(),
                    "width": restated.width,
                    "height": restated.height,
                    "decodeMs": restated.decode_ms,
                    "encodeMs": elapsed_ms - restated.decode_ms,
                    "elapsedMs": elapsed_ms,
                }));
                Some(Bytes::from(restated.png))
            }
            Err(reason) => {
                tracing::warn!(
                    path = path.as_str(),
                    format = kind.media_type(),
                    reason,
                    "image could not be restated as PNG; serving it as authored"
                );
                None
            }
        }
    })
    .await
    .ok()
    .flatten()
}

/// Whether an image of this size and shape may be decoded at all.
///
/// Checked against the container's own declared dimensions, before either decoder
/// allocates, so an oversized image is refused rather than survived.
fn within_decode_budget(width: u64, height: u64, bytes_per_pixel: u64) -> Result<(), &'static str> {
    within_budget(
        width,
        height,
        bytes_per_pixel,
        adapted_image_decode_budget(),
    )
}

fn within_budget(
    width: u64,
    height: u64,
    bytes_per_pixel: u64,
    budget: u64,
) -> Result<(), &'static str> {
    if width == 0 || height == 0 {
        return Err("image has an empty dimension");
    }
    let decoded = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or("image dimensions overflow")?;
    if decoded > budget {
        return Err("image exceeds the decode memory budget");
    }
    Ok(())
}

fn avif_as_png(bytes: &[u8]) -> Result<RestatedImage, &'static str> {
    use avif_decode::{Decoder, Image};

    let (width, height, bytes_per_pixel) = avif_decode_cost(bytes)?;
    within_decode_budget(width, height, bytes_per_pixel)?;

    let started = std::time::Instant::now();
    let decoded = Decoder::from_avif(bytes)
        .map_err(|_| "the AVIF container could not be read")?
        .to_image()
        .map_err(|_| "the AVIF image could not be decoded")?;
    let decode_ms = milliseconds(started);
    let mut restated = match decoded {
        Image::Rgb8(image) => encode_rows(image, ColorType::Rgb, BitDepth::Eight),
        Image::Rgba8(image) => encode_rows(image, ColorType::Rgba, BitDepth::Eight),
        Image::Gray8(image) => encode_rows(image, ColorType::Grayscale, BitDepth::Eight),
        Image::Rgb16(image) => encode_rows_16(image, ColorType::Rgb),
        Image::Rgba16(image) => encode_rows_16(image, ColorType::Rgba),
        Image::Gray16(image) => encode_rows_16(image, ColorType::Grayscale),
    }?;
    restated.decode_ms = decode_ms;
    Ok(restated)
}

/// What decoding this AVIF will cost, read from the container rather than guessed.
///
/// The size comes from the AV1 sequence header, which is what rav1d will actually
/// allocate for, rather than from the `ispe` box, which only declares a display size.
/// Alpha arrives as a second AV1 item and so adds a sample per pixel.
fn avif_decode_cost(bytes: &[u8]) -> Result<(u64, u64, u64), &'static str> {
    let mut parsed = bytes;
    let container =
        avif_parse::read_avif(&mut parsed).map_err(|_| "the AVIF container could not be read")?;
    let primary = container
        .primary_item_metadata()
        .map_err(|_| "the AVIF image has no readable sequence header")?;
    let alpha = container.alpha_item_metadata().ok().flatten().is_some();
    let samples = u64::from(if primary.monochrome { 1_u8 } else { 3 }) + u64::from(alpha);
    let sample_bytes = if primary.bit_depth > 8 { 2 } else { 1 };
    Ok((
        u64::from(primary.max_frame_width.get()),
        u64::from(primary.max_frame_height.get()),
        samples * sample_bytes,
    ))
}

fn milliseconds(since: std::time::Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

fn encode_rows<P: bytemuck::Pod>(
    image: imgref::ImgVec<P>,
    color: ColorType,
    depth: BitDepth,
) -> Result<RestatedImage, &'static str> {
    write_png(
        image.width() as u32,
        image.height() as u32,
        color,
        depth,
        |sink| {
            for row in image.rows() {
                sink.write_all(bytemuck::cast_slice(row))?;
            }
            Ok(())
        },
    )
}

fn encode_rows_16<P: bytemuck::Pod>(
    image: imgref::ImgVec<P>,
    color: ColorType,
) -> Result<RestatedImage, &'static str> {
    write_png(
        image.width() as u32,
        image.height() as u32,
        color,
        BitDepth::Sixteen,
        |sink| {
            // PNG stores samples wider than a byte most significant byte first.
            let mut row_bytes = Vec::new();
            for row in image.rows() {
                row_bytes.clear();
                for sample in bytemuck::cast_slice::<P, u16>(row) {
                    row_bytes.extend_from_slice(&sample.to_be_bytes());
                }
                sink.write_all(&row_bytes)?;
            }
            Ok(())
        },
    )
}

fn jxl_as_png(bytes: &[u8]) -> Result<RestatedImage, &'static str> {
    let mut image = JxlImage::builder()
        .read(bytes)
        .map_err(|_| "the JPEG XL image could not be read")?;
    // Rendering is requested in sRGB so the samples can be written without carrying a
    // colour profile the engine would then have to honour.
    image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
    let width = image.width() as usize;
    let height = image.height() as usize;
    // A render materializes f32 samples, so a channel costs four bytes here rather than
    // the one the PNG will carry.
    within_decode_budget(
        image.width() as u64,
        image.height() as u64,
        image.pixel_format().channels() as u64 * 4,
    )?;
    if image.num_loaded_frames() == 0 {
        return Err("the JPEG XL image carries no frame");
    }
    let started = std::time::Instant::now();
    let render = image
        .render_frame(0)
        .map_err(|_| "the JPEG XL frame could not be rendered")?;
    let frame = render.image_all_channels();
    let channels = frame.channels();
    let color = match channels {
        1 => ColorType::Grayscale,
        2 => ColorType::GrayscaleAlpha,
        3 => ColorType::Rgb,
        4 => ColorType::Rgba,
        _ => return Err("the JPEG XL image has an unsupported channel count"),
    };
    let samples = frame.buf();
    if samples.len() != width * height * channels {
        return Err("the JPEG XL frame has an unexpected sample count");
    }
    let decode_ms = milliseconds(started);
    let mut restated = write_png(
        width as u32,
        height as u32,
        color,
        BitDepth::Eight,
        |sink| {
            // Rendering yields normalized samples, which PNG carries as 8 bit integers.
            let mut row = Vec::with_capacity(width * channels);
            for line in samples.chunks_exact(width * channels) {
                row.clear();
                row.extend(
                    line.iter()
                        .map(|sample| (sample.clamp(0.0, 1.0) * 255.0).round() as u8),
                );
                sink.write_all(&row)?;
            }
            Ok(())
        },
    )?;
    restated.decode_ms = decode_ms;
    Ok(restated)
}

/// Encodes a PNG by writing rows straight into the compressor.
///
/// A reading device has little memory to spare, so the decoded bitmap is never copied
/// into a second buffer the size of the image just to hand it over.
fn write_png(
    width: u32,
    height: u32,
    color: ColorType,
    depth: BitDepth,
    rows: impl FnOnce(&mut dyn std::io::Write) -> std::io::Result<()>,
) -> Result<RestatedImage, &'static str> {
    let mut out = Vec::new();
    {
        let mut encoder = Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        let mut writer = encoder
            .write_header()
            .map_err(|_| "the PNG header could not be written")?;
        let mut sink = writer
            .stream_writer()
            .map_err(|_| "the PNG body could not be started")?;
        rows(&mut sink).map_err(|_| "the PNG body could not be written")?;
        sink.finish()
            .map_err(|_| "the PNG rows could not be closed")?;
        writer
            .finish()
            .map_err(|_| "the PNG could not be finished")?;
    }
    Ok(RestatedImage {
        png: out,
        width,
        height,
        decode_ms: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::{AdaptableImage, adaptable_image, image_adaptation, request_names_image_type};
    use axum::http::{HeaderMap, HeaderValue, header::ACCEPT};

    fn headers(accept: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_str(accept).unwrap());
        headers
    }

    const PROBE_AVIF: &[u8] = include_bytes!(
        "../../../../../tests/fixtures/engine-support/elephant-engine-support-src/EPUB/media/probe.avif"
    );
    const PROBE_JXL: &[u8] = include_bytes!(
        "../../../../../tests/fixtures/engine-support/elephant-engine-support-src/EPUB/media/probe.jxl"
    );

    /// A PNG the engine cannot decode is worse than no conversion at all, so both
    /// decoders are checked by reading their output back.
    #[test]
    fn both_decoders_produce_a_png_that_decodes_to_the_authored_size() {
        for (label, bytes, encoded) in [
            ("avif", PROBE_AVIF, super::avif_as_png(PROBE_AVIF)),
            ("jxl", PROBE_JXL, super::jxl_as_png(PROBE_JXL)),
        ] {
            assert!(!bytes.is_empty(), "{label} sample is missing");
            let encoded = encoded.unwrap_or_else(|reason| panic!("{label}: {reason}"));
            let decoder = png::Decoder::new(std::io::Cursor::new(&encoded.png));
            let mut reader = decoder
                .read_info()
                .unwrap_or_else(|error| panic!("{label} png header: {error}"));
            let mut restored = vec![0; reader.output_buffer_size().unwrap()];
            let info = reader
                .next_frame(&mut restored)
                .unwrap_or_else(|error| panic!("{label} png body: {error}"));
            assert_eq!((info.width, info.height), (4, 4), "{label} size");
            assert_eq!(
                (encoded.width, encoded.height),
                (4, 4),
                "{label} reported size"
            );
        }
    }

    /// The budget is bytes rather than pixels, so what it allows depends on the image.
    #[test]
    fn the_decode_budget_allows_far_more_pixels_of_a_cheaper_image() {
        const BUDGET: u64 = 384 * 1024 * 1024;
        let within_decode_budget = |width: u64, height: u64, bytes: u64| {
            super::within_budget(width, height, bytes, BUDGET)
        };

        let megapixels = |bytes_per_pixel: u64| {
            let mut low = 1_u64;
            let mut high = 2_000_000_000_u64;
            while low < high {
                let middle = low + (high - low).div_ceil(2);
                if within_decode_budget(middle, 1, bytes_per_pixel).is_ok() {
                    low = middle;
                } else {
                    high = middle - 1;
                }
            }
            low / 1_000_000
        };

        // Eight bit grayscale, eight bit RGB, eight bit RGBA, sixteen bit RGBA, and a
        // JPEG XL render with alpha, which materializes four bytes per channel.
        assert_eq!(megapixels(1), 402);
        assert_eq!(megapixels(3), 134);
        assert_eq!(megapixels(4), 100);
        assert_eq!(megapixels(8), 50);
        assert_eq!(megapixels(16), 25);

        // A 300 dpi double page A4 spread is about 17 megapixels and has to fit.
        assert!(within_decode_budget(4960, 3508, 4).is_ok());
        // A 600 dpi spread of the same pages is about 70 megapixels.
        assert!(within_decode_budget(9921, 7016, 4).is_ok());
    }

    #[test]
    fn an_empty_or_overflowing_size_is_refused_rather_than_decoded() {
        let within_decode_budget = |width: u64, height: u64, bytes: u64| {
            super::within_budget(width, height, bytes, 384 * 1024 * 1024)
        };

        assert!(within_decode_budget(0, 100, 4).is_err());
        assert!(within_decode_budget(100, 0, 4).is_err());
        assert!(within_decode_budget(u64::MAX, u64::MAX, 4).is_err());
    }

    /// The cost has to be readable from the container, so an oversized image can be
    /// refused before rav1d allocates for it.
    #[test]
    fn an_avif_states_its_decode_cost_before_anything_is_decoded() {
        let (width, height, bytes_per_pixel) = super::avif_decode_cost(PROBE_AVIF).unwrap();
        // The sample is a four pixel square, eight bit, no alpha and not monochrome.
        assert_eq!((width, height), (4, 4));
        assert_eq!(bytes_per_pixel, 3);
        assert!(super::within_decode_budget(width, height, bytes_per_pixel).is_ok());

        // The same shape at sixty thousand pixels a side is refused on any budget this
        // host would choose, since the ceiling is a gigabyte.
        assert_eq!(
            super::within_decode_budget(60_000, 60_000, bytes_per_pixel).err(),
            Some("image exceeds the decode memory budget")
        );
        assert_eq!(
            super::avif_decode_cost(b"not an AVIF container").err(),
            Some("the AVIF container could not be read")
        );
    }

    #[test]
    fn only_the_two_undecodable_core_image_types_are_adaptable() {
        assert_eq!(adaptable_image("image/avif"), Some(AdaptableImage::Avif));
        assert_eq!(adaptable_image("image/jxl"), Some(AdaptableImage::Jxl));
        for decoded in ["image/png", "image/jpeg", "image/webp", "image/svg+xml"] {
            assert_eq!(adaptable_image(decoded), None);
        }
    }

    #[test]
    fn a_wildcard_is_not_evidence_that_the_engine_decodes_a_format() {
        // The Android System WebView asks for images exactly this way.
        let webview = headers("image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8");
        assert!(!request_names_image_type(&webview, "image/avif"));
        assert!(request_names_image_type(&webview, "image/webp"));

        // Desktop Chromium adds the one format it has a decoder for.
        let desktop = headers("image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8");
        assert!(request_names_image_type(&desktop, "image/avif"));
    }

    #[test]
    fn an_engine_that_names_the_format_is_served_the_authored_bytes() {
        let desktop = headers("image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8");
        assert_eq!(image_adaptation(&desktop, "image/avif"), None);
        assert_eq!(
            image_adaptation(&desktop, "image/jxl"),
            Some(AdaptableImage::Jxl)
        );

        let webview = headers("image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8");
        assert_eq!(
            image_adaptation(&webview, "image/avif"),
            Some(AdaptableImage::Avif)
        );
        assert_eq!(image_adaptation(&webview, "image/png"), None);
    }

    #[test]
    fn a_request_without_an_accept_header_names_nothing() {
        let bare = HeaderMap::new();
        assert!(!request_names_image_type(&bare, "image/avif"));
        assert_eq!(
            image_adaptation(&bare, "image/avif"),
            Some(AdaptableImage::Avif)
        );
    }

    #[test]
    fn parameters_and_spacing_do_not_hide_a_named_format() {
        let spaced = headers("image/avif; q=0.9 , image/webp");
        assert!(request_names_image_type(&spaced, "image/avif"));
        assert!(request_names_image_type(&spaced, "image/webp"));
        let uppercase = headers("IMAGE/AVIF");
        assert!(request_names_image_type(&uppercase, "image/avif"));
    }
}
