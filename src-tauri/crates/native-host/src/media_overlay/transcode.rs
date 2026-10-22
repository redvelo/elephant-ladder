use std::cell::Cell;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;
use std::rc::Rc;

use ogg::reading::PacketReader;
use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use opus::{Application, Bitrate, Channels};
use rubato::audioadapter_buffers::direct::{InterleavedSlice, SequentialSliceOfVecs};
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::audio::well_known::{CODEC_ID_AAC, CODEC_ID_OPUS};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::well_known::{FORMAT_ID_ADTS, FORMAT_ID_ISOMP4};
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Opus codes at 48 kHz; every other source rate is resampled to it.
const OPUS_RATE: u32 = 48_000;
/// 20 ms frames, the usual trade-off between packet overhead and seek granularity.
const OPUS_FRAME_SAMPLES: usize = 960;
/// libopus' default encoder lookahead at 48 kHz, which the decoder discards.
const OPUS_PRE_SKIP: u16 = 312;
const OPUS_STREAM_SERIAL: u32 = 0x5350_554f;
const MAX_OPUS_PACKET_BYTES: usize = 4_000;
/// Speech stays transparent well below this; a 15 hour audiobook lands near 200 MB.
const OPUS_BITRATE_PER_CHANNEL: i32 = 24_000;
const RESAMPLER_CHUNK_FRAMES: usize = 1_024;

#[derive(Debug)]
pub(crate) enum TranscodeError {
    Decode(String),
    Encode(String),
    LimitExceeded,
    Io(io::Error),
}

impl From<io::Error> for TranscodeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub(crate) struct TranscodedAudio {
    pub hash: String,
    pub length: u64,
}

/// A core EPUB audio type whose playability differs between the engines this app ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceAudio {
    /// AAC, in MP4 or ADTS. Desktop CEF ships without it.
    Aac,
    /// Opus in Ogg. The Android System WebView ships without it.
    OggOpus,
    /// Opus in MP4, a core type since EPUB 3.4 that no engine here plays: CEF cannot
    /// demux MP4 at all and the Android System WebView cannot decode Opus.
    OpusInMp4,
}

/// The format a sidecar is encoded into so the engine can play it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioTarget {
    OggOpus,
    Mp3,
}

impl AudioTarget {
    pub(crate) fn media_type(self) -> &'static str {
        match self {
            Self::OggOpus => "audio/ogg",
            Self::Mp3 => "audio/mpeg",
        }
    }

    /// Names the encoder settings so an installed sidecar is reused only for an
    /// identical conversion.
    pub(crate) fn derivation(self) -> &'static str {
        match self {
            Self::OggOpus => "opus48-v1",
            Self::Mp3 => "mp3-64-v1",
        }
    }
}

/// Which of the two formats this file is, by reading the container rather than the name.
pub(crate) fn source_audio(path: &Path) -> Option<SourceAudio> {
    if is_ogg_opus(path) {
        return Some(SourceAudio::OggOpus);
    }
    let format = open_format(path).ok()?;
    let container = format.format_info().format;
    let track_codec = |wanted| {
        format.tracks().iter().any(|track| {
            matches!(&track.codec_params, Some(CodecParameters::Audio(params)) if params.codec == wanted)
        })
    };
    // An MP4 may hold either, and which one it is decides whether an engine that demuxes
    // MP4 can play it, so the codec is read before the container.
    if container == FORMAT_ID_ISOMP4 && track_codec(CODEC_ID_OPUS) {
        return Some(SourceAudio::OpusInMp4);
    }
    let aac =
        container == FORMAT_ID_ISOMP4 || container == FORMAT_ID_ADTS || track_codec(CODEC_ID_AAC);
    aac.then_some(SourceAudio::Aac)
}

/// An Ogg stream whose first packet is an Opus identification header.
///
/// Symphonia is built here without the Ogg reader and has no Opus decoder at all, so
/// the container is recognized from its own bytes.
fn is_ogg_opus(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut reader = PacketReader::new(BufReader::new(file));
    matches!(reader.read_packet(), Ok(Some(packet)) if packet.data.starts_with(b"OpusHead"))
}

/// Re-encodes a lossy source into Ogg Opus. The source is already lossy, so preserving its exact
/// samples buys nothing; this keeps a long audiobook in the hundreds of megabytes instead of
/// several gigabytes of the user's disk.
pub(crate) fn transcode_to_opus(
    source: &Path,
    output: &mut File,
    limit: u64,
) -> Result<TranscodedAudio, TranscodeError> {
    let written = Rc::new(Cell::new(0_u64));
    let mut packets = PacketWriter::new(HashingWriter {
        inner: BufWriter::new(output),
        hasher: blake3::Hasher::new(),
        length: Rc::clone(&written),
    });
    let mut stream: Option<OpusStream> = None;
    decode_source(source, &mut |rate, channels, samples| {
        let stream = match &mut stream {
            Some(stream) => stream,
            None => stream.insert(OpusStream::new(rate, channels, &mut packets)?),
        };
        if stream.source_rate != rate || stream.source_channels != channels {
            return Err(TranscodeError::Decode(
                "audio format changed within the stream".to_owned(),
            ));
        }
        stream.push(samples, &mut packets, limit, &written)
    })?;
    let mut stream = stream.ok_or_else(|| TranscodeError::Decode("no decoded audio".to_owned()))?;
    stream.finish(&mut packets, limit, &written)?;
    let mut hashing = packets.into_inner();
    hashing.flush()?;
    if hashing.length.get() > limit {
        return Err(TranscodeError::LimitExceeded);
    }
    Ok(TranscodedAudio {
        hash: hashing.hasher.finalize().to_hex().to_string(),
        length: hashing.length.get(),
    })
}

type OggPackets<'a> = PacketWriter<'static, HashingWriter<BufWriter<&'a mut File>>>;

struct OpusStream {
    source_rate: u32,
    source_channels: usize,
    channels: usize,
    encoder: opus::Encoder,
    resampler: Option<Fft<f32>>,
    resampled: Vec<f32>,
    planar: Vec<Vec<f32>>,
    ready: Vec<f32>,
    packet: Vec<u8>,
    granule: u64,
}

impl OpusStream {
    fn new(
        rate: u32,
        source_channels: usize,
        packets: &mut OggPackets<'_>,
    ) -> Result<Self, TranscodeError> {
        if source_channels == 0 {
            return Err(TranscodeError::Decode("audio has no channels".to_owned()));
        }
        let channels = if source_channels == 1 { 1 } else { 2 };
        let mut encoder = opus::Encoder::new(
            OPUS_RATE,
            if channels == 1 {
                Channels::Mono
            } else {
                Channels::Stereo
            },
            Application::Audio,
        )
        .map_err(encode_error)?;
        encoder
            .set_bitrate(Bitrate::Bits(channels as i32 * OPUS_BITRATE_PER_CHANNEL))
            .map_err(encode_error)?;
        let resampler = if rate == OPUS_RATE {
            None
        } else {
            Some(
                Fft::<f32>::new(
                    rate as usize,
                    OPUS_RATE as usize,
                    RESAMPLER_CHUNK_FRAMES,
                    channels,
                    FixedSync::Input,
                )
                .map_err(encode_error)?,
            )
        };
        write_header_pages(packets, channels, rate)?;
        Ok(Self {
            source_rate: rate,
            source_channels,
            channels,
            encoder,
            resampler,
            resampled: Vec::new(),
            planar: vec![Vec::new(); channels],
            ready: Vec::new(),
            packet: vec![0; MAX_OPUS_PACKET_BYTES],
            granule: u64::from(OPUS_PRE_SKIP),
        })
    }

    fn push(
        &mut self,
        interleaved: &[f32],
        packets: &mut OggPackets<'_>,
        limit: u64,
        written: &Cell<u64>,
    ) -> Result<(), TranscodeError> {
        for frame in interleaved.chunks_exact(self.source_channels) {
            if self.channels == 1 {
                self.planar[0].push(frame[0]);
            } else {
                let (mut left, mut right, mut lefts, mut rights) = (0.0, 0.0, 0, 0);
                for (index, sample) in frame.iter().enumerate() {
                    if index % 2 == 0 {
                        left += sample;
                        lefts += 1;
                    } else {
                        right += sample;
                        rights += 1;
                    }
                }
                self.planar[0].push(left / lefts.max(1) as f32);
                self.planar[1].push(right / rights.max(1) as f32);
            }
        }
        self.drain_resampler()?;
        self.encode_ready(packets, limit, written, false)
    }

    fn drain_resampler(&mut self) -> Result<(), TranscodeError> {
        let Some(resampler) = &mut self.resampler else {
            let frames = self.planar[0].len();
            for index in 0..frames {
                for channel in &self.planar {
                    self.ready.push(channel[index]);
                }
            }
            for channel in &mut self.planar {
                channel.clear();
            }
            return Ok(());
        };
        loop {
            let needed = resampler.input_frames_next();
            if self.planar[0].len() < needed {
                return Ok(());
            }
            let produced = resample_chunk(
                resampler,
                &self.planar,
                needed,
                self.channels,
                &mut self.resampled,
            )?;
            for channel in &mut self.planar {
                channel.drain(..needed);
            }
            self.ready
                .extend_from_slice(&self.resampled[..produced * self.channels]);
        }
    }

    fn encode_ready(
        &mut self,
        packets: &mut OggPackets<'_>,
        limit: u64,
        written: &Cell<u64>,
        last: bool,
    ) -> Result<(), TranscodeError> {
        let block = OPUS_FRAME_SAMPLES * self.channels;
        while self.ready.len() >= block {
            let encoded = self
                .encoder
                .encode_float(&self.ready[..block], &mut self.packet)
                .map_err(encode_error)?;
            self.ready.drain(..block);
            self.granule += OPUS_FRAME_SAMPLES as u64;
            let end = if last && self.ready.is_empty() {
                PacketWriteEndInfo::EndStream
            } else {
                PacketWriteEndInfo::NormalPacket
            };
            packets.write_packet(
                self.packet[..encoded].to_vec(),
                OPUS_STREAM_SERIAL,
                end,
                self.granule,
            )?;
            if written.get() > limit {
                return Err(TranscodeError::LimitExceeded);
            }
        }
        Ok(())
    }

    fn finish(
        &mut self,
        packets: &mut OggPackets<'_>,
        limit: u64,
        written: &Cell<u64>,
    ) -> Result<(), TranscodeError> {
        if let Some(resampler) = &mut self.resampler {
            if !self.planar[0].is_empty() {
                let needed = resampler.input_frames_next();
                for channel in &mut self.planar {
                    channel.resize(needed, 0.0);
                }
                let produced = resample_chunk(
                    resampler,
                    &self.planar,
                    needed,
                    self.channels,
                    &mut self.resampled,
                )?;
                for channel in &mut self.planar {
                    channel.clear();
                }
                self.ready
                    .extend_from_slice(&self.resampled[..produced * self.channels]);
            }
        } else {
            self.drain_resampler()?;
        }
        let block = OPUS_FRAME_SAMPLES * self.channels;
        if !self.ready.is_empty() {
            let padded = self.ready.len().div_ceil(block) * block;
            self.ready.resize(padded, 0.0);
        } else {
            // An empty tail still needs a terminating page.
            self.ready.resize(block, 0.0);
        }
        self.encode_ready(packets, limit, written, true)
    }
}

fn write_header_pages(
    packets: &mut OggPackets<'_>,
    channels: usize,
    source_rate: u32,
) -> Result<(), TranscodeError> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(channels as u8);
    head.extend_from_slice(&OPUS_PRE_SKIP.to_le_bytes());
    head.extend_from_slice(&source_rate.to_le_bytes());
    head.extend_from_slice(&0_i16.to_le_bytes());
    head.push(0);
    packets.write_packet(head, OPUS_STREAM_SERIAL, PacketWriteEndInfo::EndPage, 0)?;

    let vendor = b"elephant-ladder";
    let mut tags = Vec::with_capacity(8 + 4 + vendor.len() + 4);
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0_u32.to_le_bytes());
    packets.write_packet(tags, OPUS_STREAM_SERIAL, PacketWriteEndInfo::EndPage, 0)?;
    Ok(())
}

/// Resamples one chunk of planar input into `output`, returning the frames produced.
///
/// The resampler reads and writes through adapters over buffers this module already owns,
/// so a chunk costs no allocation of its own.
fn resample_chunk(
    resampler: &mut Fft<f32>,
    planar: &[Vec<f32>],
    frames: usize,
    channels: usize,
    output: &mut Vec<f32>,
) -> Result<usize, TranscodeError> {
    let input = SequentialSliceOfVecs::new(planar, channels, frames)
        .map_err(|error| TranscodeError::Encode(error.to_string()))?;
    let capacity = resampler.output_frames_next() * channels;
    if output.len() < capacity {
        output.resize(capacity, 0.0);
    }
    let mut sink = InterleavedSlice::new_mut(output.as_mut_slice(), channels, capacity / channels)
        .map_err(|error| TranscodeError::Encode(error.to_string()))?;
    let (_, produced) = resampler
        .process_into_buffer(&input, &mut sink, None)
        .map_err(encode_error)?;
    Ok(produced)
}

fn open_format(path: &Path) -> Result<Box<dyn FormatReader>, SymphoniaError> {
    let file = File::open(path)?;
    symphonia::default::get_probe().probe(
        &Hint::new(),
        MediaSourceStream::new(Box::new(file), Default::default()),
        FormatOptions::default().prebuild_seek_index(false),
        MetadataOptions::default(),
    )
}

fn decode_error(error: SymphoniaError) -> TranscodeError {
    match error {
        SymphoniaError::IoError(error) => TranscodeError::Io(error),
        error => TranscodeError::Decode(error.to_string()),
    }
}

fn encode_error(error: impl std::fmt::Display) -> TranscodeError {
    TranscodeError::Encode(error.to_string())
}

struct HashingWriter<W> {
    inner: W,
    hasher: blake3::Hasher,
    length: Rc<Cell<u64>>,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.hasher.update(&buffer[..written]);
        self.length.set(self.length.get() + written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Re-encodes a lossy source as MP3, the one audio format every engine this app runs on
/// decodes. The source is already lossy, so its exact samples are not worth preserving.
pub(crate) fn transcode_to_mp3(
    source: &Path,
    output: &mut File,
    limit: u64,
) -> Result<TranscodedAudio, TranscodeError> {
    let written = Rc::new(Cell::new(0_u64));
    let mut sink = HashingWriter {
        inner: BufWriter::new(output),
        hasher: blake3::Hasher::new(),
        length: Rc::clone(&written),
    };
    let mut stream: Option<Mp3Stream> = None;
    decode_source(source, &mut |rate, channels, samples| {
        let stream = match &mut stream {
            Some(stream) => stream,
            None => stream.insert(Mp3Stream::new(rate, channels)?),
        };
        if stream.rate != rate || stream.channels != channels {
            return Err(TranscodeError::Decode(
                "audio format changed within the stream".to_owned(),
            ));
        }
        stream.push(samples, &mut sink)?;
        if written.get() > limit {
            return Err(TranscodeError::LimitExceeded);
        }
        Ok(())
    })?;
    let mut stream = stream.ok_or_else(|| TranscodeError::Decode("no decoded audio".to_owned()))?;
    stream.finish(&mut sink)?;
    sink.flush()?;
    if written.get() > limit {
        return Err(TranscodeError::LimitExceeded);
    }
    Ok(TranscodedAudio {
        hash: sink.hasher.finalize().to_hex().to_string(),
        length: written.get(),
    })
}

type SampleSink<'a> = &'a mut dyn FnMut(u32, usize, &[f32]) -> Result<(), TranscodeError>;

/// Decodes either source format into interleaved samples, whichever encoder wants them.
fn decode_source(source: &Path, sink: SampleSink<'_>) -> Result<(), TranscodeError> {
    match source_audio(source) {
        Some(SourceAudio::OggOpus) => decode_ogg_opus(source, sink),
        Some(SourceAudio::OpusInMp4) => decode_mp4_opus(source, sink),
        Some(SourceAudio::Aac) | None => decode_with_symphonia(source, sink),
    }
}

/// Decodes Opus carried in MP4, which Symphonia demuxes but has no decoder for.
///
/// The sample entry hands over the same identification header Ogg carries, except that
/// its fields are big endian here, being an ISO box, and that the magic may be absent.
/// Each packet is then one bare Opus frame, which is what libopus wants.
fn decode_mp4_opus(source: &Path, sink: SampleSink<'_>) -> Result<(), TranscodeError> {
    let mut format = open_format(source).map_err(decode_error)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| TranscodeError::Decode("no audio track".to_owned()))?;
    let track_id = track.id;
    let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
        return Err(TranscodeError::Decode(
            "audio track has no codec parameters".to_owned(),
        ));
    };
    let identification = params.extra_data.as_deref().unwrap_or_default();
    let header = identification
        .strip_prefix(b"OpusHead".as_slice())
        .unwrap_or(identification);
    if header.len() < 4 {
        return Err(TranscodeError::Decode(
            "the Opus identification header is truncated".to_owned(),
        ));
    }
    let channels = match header[1] {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        other => {
            return Err(TranscodeError::Decode(format!(
                "Opus audio has {other} channels"
            )));
        }
    };
    let channel_count = if channels == Channels::Mono { 1 } else { 2 };
    let mut remaining_pre_skip = usize::from(u16::from_be_bytes([header[2], header[3]]));
    let mut decoder =
        opus::Decoder::new(OPUS_RATE, channels).map_err(|error| opus_error("decoder", error))?;
    let mut decoded = vec![0_f32; OPUS_MAX_FRAME_SAMPLES * channel_count];
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(error) => return Err(decode_error(error)),
        };
        if packet.track_id != track_id {
            continue;
        }
        let frames = decoder
            .decode_float(&packet.data, &mut decoded, false)
            .map_err(|error| opus_error("packet", error))?;
        let skipped = remaining_pre_skip.min(frames);
        remaining_pre_skip -= skipped;
        let samples = &decoded[skipped * channel_count..frames * channel_count];
        if samples.is_empty() {
            continue;
        }
        sink(OPUS_RATE, channel_count, samples)?;
    }
    Ok(())
}

fn decode_with_symphonia(source: &Path, sink: SampleSink<'_>) -> Result<(), TranscodeError> {
    let mut format = open_format(source).map_err(decode_error)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| TranscodeError::Decode("no audio track".to_owned()))?;
    let track_id = track.id;
    let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
        return Err(TranscodeError::Decode(
            "audio track has no codec parameters".to_owned(),
        ));
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(decode_error)?;
    let mut interleaved = Vec::<f32>::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(error) => return Err(decode_error(error)),
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = decoder.decode(&packet).map_err(decode_error)?;
        let rate = decoded.spec().rate();
        let channels = decoded.spec().channels().count();
        decoded.copy_to_vec_interleaved::<f32>(&mut interleaved);
        sink(rate, channels, &interleaved)?;
    }
    Ok(())
}

/// Opus always decodes at 48 kHz, and its longest packet is 120 ms.
const OPUS_MAX_FRAME_SAMPLES: usize = 5_760;

fn decode_ogg_opus(source: &Path, sink: SampleSink<'_>) -> Result<(), TranscodeError> {
    let file = File::open(source)?;
    let mut reader = PacketReader::new(BufReader::new(file));
    let header = reader
        .read_packet()
        .map_err(ogg_error)?
        .filter(|packet| packet.data.starts_with(b"OpusHead"))
        .ok_or_else(|| TranscodeError::Decode("no Opus identification header".to_owned()))?;
    if header.data.len() < 12 {
        return Err(TranscodeError::Decode(
            "the Opus identification header is truncated".to_owned(),
        ));
    }
    let channels = match header.data[9] {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        other => {
            return Err(TranscodeError::Decode(format!(
                "Opus audio has {other} channels"
            )));
        }
    };
    let channel_count = if channels == Channels::Mono { 1 } else { 2 };
    // The encoder's lookahead is not part of the recording and the decoder discards it.
    let mut remaining_pre_skip =
        usize::from(u16::from_le_bytes([header.data[10], header.data[11]]));
    let mut decoder =
        opus::Decoder::new(OPUS_RATE, channels).map_err(|error| opus_error("decoder", error))?;
    let mut decoded = vec![0_f32; OPUS_MAX_FRAME_SAMPLES * channel_count];
    let mut first = true;
    while let Some(packet) = reader.read_packet().map_err(ogg_error)? {
        // The comment header follows the identification header and carries no audio.
        if first {
            first = false;
            if packet.data.starts_with(b"OpusTags") {
                continue;
            }
        }
        let frames = decoder
            .decode_float(&packet.data, &mut decoded, false)
            .map_err(|error| opus_error("packet", error))?;
        let skipped = remaining_pre_skip.min(frames);
        remaining_pre_skip -= skipped;
        let samples = &decoded[skipped * channel_count..frames * channel_count];
        if samples.is_empty() {
            continue;
        }
        sink(OPUS_RATE, channel_count, samples)?;
    }
    Ok(())
}

fn ogg_error(error: ogg::OggReadError) -> TranscodeError {
    match error {
        ogg::OggReadError::ReadError(error) => TranscodeError::Io(error),
        error => TranscodeError::Decode(error.to_string()),
    }
}

fn opus_error(stage: &str, error: opus::Error) -> TranscodeError {
    TranscodeError::Decode(format!("Opus {stage}: {error}"))
}

struct Mp3Stream {
    rate: u32,
    channels: usize,
    encoder: mp3lame_encoder::Encoder,
    encoded: Vec<u8>,
}

impl Mp3Stream {
    fn new(rate: u32, channels: usize) -> Result<Self, TranscodeError> {
        use mp3lame_encoder::{Bitrate, Builder, Mode, Quality};

        let mut builder =
            Builder::new().ok_or_else(|| TranscodeError::Encode("no MP3 encoder".to_owned()))?;
        let encode_error =
            |error: mp3lame_encoder::BuildError| TranscodeError::Encode(error.to_string());
        builder.set_sample_rate(rate).map_err(encode_error)?;
        builder
            .set_num_channels(u8::try_from(channels).unwrap_or(2))
            .map_err(encode_error)?;
        // Narration is speech, which stays clear well below transparency for music.
        let (bitrate, mode) = if channels == 1 {
            (Bitrate::Kbps64, Mode::Mono)
        } else {
            (Bitrate::Kbps128, Mode::JointStereo)
        };
        builder.set_brate(bitrate).map_err(encode_error)?;
        builder.set_mode(mode).map_err(encode_error)?;
        builder.set_quality(Quality::Good).map_err(encode_error)?;
        let encoder = builder.build().map_err(encode_error)?;
        Ok(Self {
            rate,
            channels,
            encoder,
            encoded: Vec::new(),
        })
    }

    fn push(&mut self, samples: &[f32], sink: &mut impl Write) -> Result<(), TranscodeError> {
        use mp3lame_encoder::{InterleavedPcm, MonoPcm, max_required_buffer_size};

        let frames = samples.len() / self.channels;
        self.encoded.clear();
        self.encoded
            .reserve(max_required_buffer_size(frames.max(1)));
        let written = if self.channels == 1 {
            self.encoder
                .encode_to_vec(MonoPcm(samples), &mut self.encoded)
        } else {
            self.encoder
                .encode_to_vec(InterleavedPcm(samples), &mut self.encoded)
        }
        .map_err(|error| TranscodeError::Encode(error.to_string()))?;
        sink.write_all(&self.encoded[..written])?;
        Ok(())
    }

    fn finish(&mut self, sink: &mut impl Write) -> Result<(), TranscodeError> {
        use mp3lame_encoder::{FlushNoGap, max_required_buffer_size};

        self.encoded.clear();
        self.encoded.reserve(max_required_buffer_size(1));
        let written = self
            .encoder
            .flush_to_vec::<FlushNoGap>(&mut self.encoded)
            .map_err(|error| TranscodeError::Encode(error.to_string()))?;
        sink.write_all(&self.encoded[..written])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioTarget, SourceAudio, source_audio, transcode_to_mp3};
    use std::io::Write;

    const PROBE_OPUS: &[u8] = include_bytes!(
        "../../../../../tests/fixtures/engine-support/elephant-engine-support-src/EPUB/media/probe.opus"
    );
    const EXCERPT_OPUS_MP4: &[u8] =
        include_bytes!("../../../../../tests/fixtures/media-overlays/moby-dick-excerpt-opus.mp4");
    const PROBE_MP3: &[u8] = include_bytes!(
        "../../../../../tests/fixtures/engine-support/elephant-engine-support-src/EPUB/media/probe.mp3"
    );

    fn staged(bytes: &[u8], extension: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("audio.{extension}"));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        (directory, path)
    }

    #[test]
    fn ogg_opus_is_recognized_from_its_container_rather_than_its_name() {
        let (_directory, path) = staged(PROBE_OPUS, "bin");
        assert_eq!(source_audio(&path), Some(SourceAudio::OggOpus));
    }

    #[test]
    fn an_audio_type_every_engine_plays_needs_no_conversion() {
        let (_directory, path) = staged(PROBE_MP3, "mp3");
        assert_eq!(source_audio(&path), None);
    }

    /// Opus in an MP4 container is its own core type and must not be mistaken for the
    /// AAC that container usually holds, since an engine that demuxes MP4 still cannot
    /// decode it.
    #[test]
    fn opus_in_mp4_is_told_apart_from_the_aac_the_container_usually_holds() {
        let (_directory, opus) = staged(EXCERPT_OPUS_MP4, "mp4");
        assert_eq!(source_audio(&opus), Some(SourceAudio::OpusInMp4));

        let (_aac_directory, aac) = staged(
            include_bytes!("../../../../../tests/fixtures/media-overlays/moby-dick-excerpt.m4a"),
            "m4a",
        );
        assert_eq!(source_audio(&aac), Some(SourceAudio::Aac));
    }

    /// Neither engine plays it, so both conversions have to work.
    #[test]
    fn opus_in_mp4_converts_to_both_targets() {
        let (_directory, source) = staged(EXCERPT_OPUS_MP4, "mp4");
        let output = tempfile::tempdir().unwrap();

        let mut mp3 = std::fs::File::create(output.path().join("narration.mp3")).unwrap();
        let converted = transcode_to_mp3(&source, &mut mp3, u64::MAX).unwrap();
        assert!(converted.length > 0);
        // Six seconds at 48 kHz, give or take the frame MP3 pads to.
        let frames = decoded_frames(output.path().join("narration.mp3"));
        assert!(
            (283_000..=292_000).contains(&frames),
            "decoded {frames} frames"
        );

        let mut ogg = std::fs::File::create(output.path().join("narration.ogg")).unwrap();
        let remuxed = super::transcode_to_opus(&source, &mut ogg, u64::MAX).unwrap();
        assert!(remuxed.length > 0);
        let bytes = std::fs::read(output.path().join("narration.ogg")).unwrap();
        assert_eq!(&bytes[..4], b"OggS");
    }

    #[test]
    fn each_target_names_its_own_media_type_and_derivation() {
        assert_eq!(AudioTarget::OggOpus.media_type(), "audio/ogg");
        assert_eq!(AudioTarget::Mp3.media_type(), "audio/mpeg");
        assert_ne!(
            AudioTarget::OggOpus.derivation(),
            AudioTarget::Mp3.derivation()
        );
    }

    /// Decodes a converted file and reports how many frames came back, so a conversion
    /// is read with a decoder rather than trusted.
    fn decoded_frames(path: std::path::PathBuf) -> u64 {
        use symphonia::core::codecs::CodecParameters;
        use symphonia::core::codecs::audio::AudioDecoderOptions;
        use symphonia::core::formats::{FormatOptions, TrackType};
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;

        let file = std::fs::File::open(&path).unwrap();
        let mut format = symphonia::default::get_probe()
            .probe(
                &symphonia::core::formats::probe::Hint::new(),
                MediaSourceStream::new(Box::new(file), Default::default()),
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .expect("the conversion is a readable container");
        let track = format.default_track(TrackType::Audio).unwrap();
        let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
            panic!("no audio codec parameters");
        };
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .expect("the conversion decodes");
        let mut frames = 0_u64;
        while let Ok(Some(packet)) = format.next_packet() {
            if let Ok(decoded) = decoder.decode(&packet) {
                frames += decoded.frames() as u64;
                assert_eq!(decoded.spec().rate(), super::OPUS_RATE);
            }
        }
        frames
    }

    /// An engine that cannot decode Opus must be handed audio it can decode, so the
    /// conversion is read back with a decoder rather than trusted.
    #[test]
    fn opus_is_converted_into_mp3_an_engine_can_decode() {
        use symphonia::core::codecs::CodecParameters;
        use symphonia::core::codecs::audio::AudioDecoderOptions;
        use symphonia::core::formats::{FormatOptions, TrackType};
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;

        let (_directory, source) = staged(PROBE_OPUS, "opus");
        let output_directory = tempfile::tempdir().unwrap();
        let output_path = output_directory.path().join("narration.mp3");
        let mut output = std::fs::File::create(&output_path).unwrap();
        let converted = transcode_to_mp3(&source, &mut output, u64::MAX).unwrap();
        drop(output);

        let written = std::fs::read(&output_path).unwrap();
        assert_eq!(converted.length, written.len() as u64);
        assert!(!written.is_empty());
        assert_eq!(converted.hash.len(), 64);

        let file = std::fs::File::open(&output_path).unwrap();
        let mut format = symphonia::default::get_probe()
            .probe(
                &symphonia::core::formats::probe::Hint::new(),
                MediaSourceStream::new(Box::new(file), Default::default()),
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .expect("the conversion is a readable container");
        let track = format.default_track(TrackType::Audio).unwrap();
        let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
            panic!("no audio codec parameters");
        };
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .expect("the conversion decodes");
        let mut frames = 0_u64;
        while let Ok(Some(packet)) = format.next_packet() {
            if let Ok(decoded) = decoder.decode(&packet) {
                frames += decoded.frames() as u64;
                assert_eq!(decoded.spec().rate(), super::OPUS_RATE);
            }
        }
        // The probe carries 80 ms of silence, and MP3 pads to its own frame size.
        assert!(frames >= 48 * 80, "decoded only {frames} frames");
    }
}
