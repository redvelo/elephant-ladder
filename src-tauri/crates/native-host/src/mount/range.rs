//! Byte ranges and response content types.
//!
//! One ordinary byte range is supported for eligible unchanged representations.
//! Prepared CSS, XHTML, and SVG are complete bounded responses and never carry a
//! range.

use crate::RESPONSE_CONTENT_TYPES;
use axum::{http::HeaderMap, http::HeaderValue, http::header::RANGE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RangeDecision {
    Full,
    Partial { start: usize, end: usize },
    Unsatisfiable,
}

pub fn single_range_header(headers: &HeaderMap) -> Option<&HeaderValue> {
    let mut values = headers.get_all(RANGE).iter();
    let first = values.next()?;
    values.next().is_none().then_some(first)
}

pub(crate) fn parse_range(header: Option<&HeaderValue>, length: usize) -> RangeDecision {
    let Some(value) = header.and_then(|value| value.to_str().ok()) else {
        return if header.is_some() {
            RangeDecision::Unsatisfiable
        } else {
            RangeDecision::Full
        };
    };
    let Some(specification) = value.strip_prefix("bytes=") else {
        return RangeDecision::Unsatisfiable;
    };
    if specification.contains(',') {
        return RangeDecision::Full;
    }
    let Some((start, end)) = specification.split_once('-') else {
        return RangeDecision::Unsatisfiable;
    };
    if start.is_empty() {
        let Ok(suffix) = end.parse::<usize>() else {
            return RangeDecision::Unsatisfiable;
        };
        if suffix == 0 || length == 0 {
            return RangeDecision::Unsatisfiable;
        }
        return RangeDecision::Partial {
            start: length.saturating_sub(suffix),
            end: length - 1,
        };
    }
    let Ok(start) = start.parse::<usize>() else {
        return RangeDecision::Unsatisfiable;
    };
    if start >= length {
        return RangeDecision::Unsatisfiable;
    }
    let end = if end.is_empty() {
        length - 1
    } else {
        let Ok(end) = end.parse::<usize>() else {
            return RangeDecision::Unsatisfiable;
        };
        end.min(length - 1)
    };
    if end < start {
        return RangeDecision::Unsatisfiable;
    }
    RangeDecision::Partial { start, end }
}

pub(crate) fn content_type(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    extension
        .as_deref()
        .and_then(|extension| {
            RESPONSE_CONTENT_TYPES
                .iter()
                .find_map(|(candidate, mime)| (*candidate == extension).then_some(*mime))
        })
        .unwrap_or("application/octet-stream")
}
