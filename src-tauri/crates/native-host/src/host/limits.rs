//! Every tunable bound the host enforces, in one table.
//!
//! The production plan's "Concurrency And Limits" section lists these values;
//! keeping them together is what makes that list diffable against the code.
//! They must be tuned from packaged target measurements rather than desktop
//! development behaviour.

use std::{sync::atomic::AtomicU64, time::Duration};

pub(crate) const READER_HOST: &str = "reader.elephant-ladder.localhost";
// Occurrence presentation gained `alignXCenter`, so stored facts no longer match what the
// current stack derives from the same EPUB. Any change to the derived facts, response rules
// or media overlay products needs this bumped: an opened publication whose snapshot was
// derived at another revision is re-derived from its stored EPUB rather than refused.
pub(crate) const PRODUCTION_READING_REVISION: &str = "reading-v21";
pub const IMAGE_DOCUMENT_QUERY: &str = "epub-navigator-image-document=1";
pub(crate) const SEARCH_REVISION: &str = "search-v4";
// How the host stages a measurement frame: its instrumentation, policy and viewport. This is
// the one measurement input no served artifact can witness, so it is bumped by hand; the agent
// and reader-font digests cover everything a publication document can observe.
pub(crate) const NATIVE_MEASUREMENT_REVISION: &str = "7";
pub(crate) const MAX_SEARCH_ENTRIES_BYTES: usize = 16 * 1_048_576;
pub(crate) const MAX_SEARCH_SNAPSHOT_BYTES: usize = 24 * 1_048_576;
pub(crate) const MAX_SEARCH_ENTRIES: usize = 100_000;
pub(crate) const MAX_SEARCH_DOCUMENTS: usize = 100_000;
pub(crate) const MAX_SEARCH_RESULT_ROWS: usize = 1_000_000;
pub(crate) const MAX_SEARCH_ENTRY_TEXT_BYTES: usize = 1_048_576;
pub(crate) const MAX_SEARCH_STRING_BYTES: usize = 16_384;
pub(crate) const MAX_SEARCH_QUERY_CODE_POINTS: usize = 512;
pub(crate) const MAX_SEARCH_QUERY_BYTES: usize = 2_048;
pub(crate) const MAX_SEARCH_CONTINUATION_BYTES: usize = 16_384;
pub(crate) const MAX_SEARCH_ID_BYTES: usize = 1_024;
pub(crate) const MAX_SEARCH_RESULTS_PER_BATCH: u32 = 100;
pub(crate) const MAX_SEARCH_REQUEST_BYTES: usize = 24 * 1024;
pub(crate) const MAX_SEARCH_EXCERPT_CODE_POINTS: usize = 512;
pub(crate) const MAX_SEARCH_EXCERPT_BYTES: usize = 2_048;
pub(crate) const MAX_FACTS_SNAPSHOT_BYTES: usize = 24 * 1_048_576;
pub(crate) const MAX_RESPONSE_MANIFEST_BYTES: usize = 24 * 1_048_576;
pub(crate) const MAX_RESPONSE_RESOURCES: usize = 100_000;
pub(crate) const MAX_RESPONSE_MIME_BYTES: usize = 256;
pub const MAX_ANALYZED_RESOURCES: usize = 10_000;
pub(crate) const MAX_RESOURCE_ANALYSIS_BYTES: u64 = 128 * 1_048_576;
pub(crate) const MAX_TOTAL_ANALYSIS_BYTES: u64 = 2 * 1024 * 1_048_576;
pub(crate) const MAX_SMIL_NODES: usize = 100_000;
pub(crate) const MAX_SMIL_NESTING: usize = 256;
pub(crate) const MAX_ACQUISITION_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub(crate) const ACQUISITION_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub(crate) const PREPARATION_WAIT: Duration = Duration::from_secs(30);
pub(crate) const MAX_READING_POSITION_TARGET_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_READING_POSITION_REQUEST_BYTES: usize = 6 * 1024 * 1024;
pub(crate) const MAX_ANNOTATION_MUTATION_REQUEST_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_PREFERENCES_REQUEST_BYTES: usize = 8 * 1024;
pub(crate) const MAX_PUBLICATION_POLICY_REQUEST_BYTES: usize = 8 * 1024;
pub(crate) const MAX_EXTERNAL_URL_REQUEST_BYTES: usize = 8 * 1024;
pub(crate) const MAX_EXTERNAL_URL_BYTES: usize = 4 * 1024;
/// A mail recipient the consent prompt has to be able to show in full.
pub(crate) const MAX_EXTERNAL_MAIL_ADDRESS_BYTES: usize = 254;
pub(crate) const TRUSTED_READER_REQUEST_HEADER: &str = "x-elephant-request";
pub(crate) const TRUSTED_READER_REQUEST_VALUE: &str = "trusted-reader";
pub const TRACE_HEADER: &str = "x-elephant-trace-id";
pub const OPEN_OPERATION_HEADER: &str = "x-elephant-open-operation";
pub const LIBRARY_DOCUMENT_HEADER: &str = "x-elephant-library-document";
pub const PAGINATION_HANDLE_HEADER: &str = "x-elephant-pagination-handle";
pub(crate) const MAX_PAGINATION_RECORD_BYTES: usize = 64 * 1024;
pub(crate) const MAX_PAGINATION_BULK_REQUEST_BYTES: usize = 4 * 1024;
pub(crate) const MAX_PAGINATION_BULK_RECORDS: usize = 32;
pub(crate) const MAX_PAGINATION_BULK_RAW_BYTES: usize =
    MAX_PAGINATION_BULK_RECORDS * MAX_PAGINATION_RECORD_BYTES;
pub(crate) const MAX_PAGINATION_BULK_ENCODED_BYTES: usize =
    MAX_PAGINATION_BULK_RECORDS * MAX_PAGINATION_RECORD_BYTES.div_ceil(3) * 4;
pub(crate) const MAX_PAGINATION_BULK_MUTATION_REQUEST_BYTES: usize =
    MAX_PAGINATION_BULK_ENCODED_BYTES + MAX_PAGINATION_BULK_RECORDS * 160 + 32;
pub(crate) const MAX_DIAGNOSTIC_BATCH_RECORDS: usize = 16;
pub(crate) const MAX_DIAGNOSTIC_REQUEST_BYTES: usize = 16 * 1024;
pub const MAX_CONFORMANCE_REQUEST_BYTES: usize = 1024 * 1024;
/// A stylesheet is read whole so EPUB's prefixed declarations can be restated; a larger
/// one is streamed as authored instead.
pub(crate) const MAX_STYLESHEET_BYTES: u64 = 2 * 1024 * 1024;
/// An image the engine cannot decode is read whole and re-encoded, so the stored bytes
/// have to fit in memory. A larger one is streamed as authored, which renders nothing
/// but costs nothing either.
pub(crate) const MAX_ADAPTED_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
/// The decoded samples one image conversion may hold, as a share of the memory the
/// machine actually has.
///
/// The bound is bytes rather than pixels because a pixel costs very different amounts:
/// one byte for an eight bit grayscale AVIF, sixteen for a JPEG XL with alpha, which
/// renders to floats. It is a share of real memory rather than a constant because the
/// same number cannot be right for a desktop and for a reader with under two gigabytes.
///
/// The share is a tenth, which leaves room for the finished PNG. That is built in memory
/// beside the decoded samples and is of the same order, so peak use is about twice this
/// and the tenth keeps it to a fifth of the machine.
const ADAPTED_IMAGE_MEMORY_SHARE: u64 = 10;
/// Below this an ordinary 300 dpi double page spread would be refused, which is worse
/// than the memory it saves.
const MIN_ADAPTED_IMAGE_DECODED_BYTES: u64 = 64 * 1024 * 1024;
// Two A4 pages at 300 dpi, eight bit and no alpha, which the floor must always admit.
const _: () = assert!(MIN_ADAPTED_IMAGE_DECODED_BYTES >= 4960 * 3508 * 3);
/// Past this the conversion is no longer what limits a publication, so a larger share of
/// a big machine buys nothing.
const MAX_ADAPTED_IMAGE_DECODED_BYTES: u64 = 1024 * 1024 * 1024;

/// The budget for a machine with this much total memory.
///
/// An unreadable total yields the floor: refusing an enormous image renders nothing,
/// which is visible and recoverable, where exhausting memory takes the whole process.
fn decode_budget_for_total_memory(total_bytes: Option<u64>) -> u64 {
    let Some(total) = total_bytes else {
        return MIN_ADAPTED_IMAGE_DECODED_BYTES;
    };
    (total / ADAPTED_IMAGE_MEMORY_SHARE).clamp(
        MIN_ADAPTED_IMAGE_DECODED_BYTES,
        MAX_ADAPTED_IMAGE_DECODED_BYTES,
    )
}

/// `MemTotal` as bytes, which Linux and Android both report here.
fn total_memory_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kilobytes: u64 = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    kilobytes.checked_mul(1024)
}

/// The decode budget for this machine, read once.
pub(crate) fn adapted_image_decode_budget() -> u64 {
    static BUDGET: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| {
        let budget = decode_budget_for_total_memory(total_memory_bytes());
        tracing::debug!(
            budget_bytes = budget,
            "image conversion decode budget for this machine"
        );
        budget
    })
}
pub(crate) const MAX_DIAGNOSTIC_COUNT: u64 = 1_000_000;
pub(crate) const MAX_DIAGNOSTIC_DURATION_MS: f64 = 3_600_000.0;
pub const TRANSFORM_CACHE_BYTES: usize = 24 * 1_048_576;
pub(crate) const TRANSFORM_CACHE_ENTRIES: usize = 256;
pub(crate) static HTTP_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
pub(crate) const MAX_TRANSFORM_SOURCE_BYTES: u64 = 8 * 1_048_576;
pub(crate) const MAX_TRANSFORMED_DOCUMENT_BYTES: usize = 9 * 1_048_576;
pub(crate) const RESOURCE_STREAM_CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const RESOURCE_STREAM_CHANNEL_CHUNKS: usize = 4;
pub const RESOURCE_STREAM_CONCURRENCY: usize = 4;
// Buffering an entry releases the shared archive lock before a slow consumer drains the body.
pub(crate) const MAX_BUFFERED_RESOURCE_BYTES: u64 = 4 * 1_048_576;
// A roll page's stand-in image and its frame fetch the same bytes moments apart, and a CORS
// request cannot share a browser cache entry with a same-origin one, so the second read is
// served from memory instead of inflating the entry again.
pub(crate) const RESOURCE_CACHE_BYTES: usize = 16 * 1_048_576;
pub(crate) const RESOURCE_CACHE_ENTRIES: usize = 8;
pub(crate) const RESPONSE_CONTENT_TYPES: &[(&str, &str)] = &[
    ("xhtml", "application/xhtml+xml; charset=utf-8"),
    ("html", "text/html; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("js", "text/javascript; charset=utf-8"),
    ("json", "application/json; charset=utf-8"),
    ("svg", "image/svg+xml"),
    ("gif", "image/gif"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("jxl", "image/jxl"),
    ("wav", "audio/wav"),
    ("mp3", "audio/mpeg"),
    ("m4a", "audio/mp4"),
    ("aac", "audio/aac"),
    ("oga", "audio/ogg"),
    ("otf", "font/otf"),
    ("ttf", "font/ttf"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("wasm", "application/wasm"),
    ("xml", "application/xml; charset=utf-8"),
    ("opf", "application/oebps-package+xml; charset=utf-8"),
];

#[cfg(test)]
mod decode_budget_tests {
    use super::{
        MAX_ADAPTED_IMAGE_DECODED_BYTES, MIN_ADAPTED_IMAGE_DECODED_BYTES,
        decode_budget_for_total_memory, total_memory_bytes,
    };

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn the_budget_is_a_tenth_of_memory_between_a_floor_and_a_ceiling() {
        // An ONYX BOOX Go6 reports about 1.74 GiB.
        let go6 = 1_827_776 * 1024;
        assert_eq!(decode_budget_for_total_memory(Some(go6)), go6 / 10);
        assert_eq!(decode_budget_for_total_memory(Some(go6)) / MIB, 178);
        // A desktop is capped, since a larger share would not change what opens.
        assert_eq!(
            decode_budget_for_total_memory(Some(64 * 1024 * MIB)),
            MAX_ADAPTED_IMAGE_DECODED_BYTES
        );
        // A small machine still gets enough for a 300 dpi double page spread.
        assert_eq!(
            decode_budget_for_total_memory(Some(512 * MIB)),
            MIN_ADAPTED_IMAGE_DECODED_BYTES
        );
    }

    #[test]
    fn an_unreadable_total_refuses_rather_than_risks_the_process() {
        assert_eq!(
            decode_budget_for_total_memory(None),
            MIN_ADAPTED_IMAGE_DECODED_BYTES
        );
    }

    #[test]
    fn this_machine_reports_its_memory() {
        let total = total_memory_bytes().expect("Linux reports MemTotal");
        assert!(total > 256 * MIB, "{total} bytes");
    }
}
