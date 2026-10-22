//! The smallest checks, shared by every other validator: clocks, paths,
//! bounded strings, index ordering, and serialized size.

use crate::media_overlay::{
    MAX_MEDIA_OVERLAY_COLLECTION_ITEMS, MAX_MEDIA_OVERLAY_STRING_BYTES, SmilClock,
};
use epub_stack::EpubPath;
#[cfg(test)]
use serde::Serialize;

pub(crate) fn validate_clock(clock: &SmilClock) -> Result<(), String> {
    if let SmilClock::Parsed { milliseconds } = clock {
        validate_string(milliseconds, false)?;
        let value = milliseconds
            .parse::<u64>()
            .map_err(|_| "SMIL clock is outside the u64 decimal domain".to_owned())?;
        if value.to_string() != *milliseconds {
            return invalid("SMIL clock is not canonical u64 decimal text");
        }
    }
    Ok(())
}

pub(crate) fn parsed_clock(clock: &SmilClock) -> Option<u64> {
    match clock {
        SmilClock::Parsed { milliseconds } => milliseconds.parse().ok(),
        SmilClock::Absent {} | SmilClock::Unrecognized {} => None,
    }
}

/// An absent `clipBegin` is the start of the audio resource.
pub(crate) fn clip_begin_clock(clock: &SmilClock) -> Option<u64> {
    match clock {
        SmilClock::Absent {} => Some(0),
        _ => parsed_clock(clock),
    }
}

pub(crate) fn validate_sorted_unique_indexes(
    indexes: &[u32],
    upper_bound: usize,
    require_nonempty: bool,
) -> Result<(), String> {
    if (require_nonempty && indexes.is_empty())
        || indexes.len() > MAX_MEDIA_OVERLAY_COLLECTION_ITEMS
        || indexes.iter().any(|index| *index as usize >= upper_bound)
        || indexes.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return invalid("index collection is empty, unsorted, duplicated, or out of range");
    }
    Ok(())
}

pub(crate) fn validate_declaration_index(
    index: u32,
    declaration_count: usize,
) -> Result<(), String> {
    if index as usize >= declaration_count {
        return invalid("declaration index is out of range");
    }
    Ok(())
}

pub(crate) fn validate_path(path: &str) -> Result<(), String> {
    validate_string(path, false)?;
    EpubPath::new(path)
        .map(|_| ())
        .map_err(|_| "media overlay path is not canonical".to_owned())
}

pub(crate) fn validate_optional_string(value: &Option<String>) -> Result<(), String> {
    if let Some(value) = value {
        validate_string(value, true)?;
    }
    Ok(())
}

pub(crate) fn validate_string(value: &str, allow_empty: bool) -> Result<(), String> {
    if (!allow_empty && value.is_empty()) || value.len() > MAX_MEDIA_OVERLAY_STRING_BYTES {
        return invalid("media overlay string is empty or exceeds its byte bound");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn validate_serialized_size<T: Serialize>(
    value: &T,
    maximum: usize,
    name: &str,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("could not serialize {name}: {error}"))?;
    if bytes.len() > maximum {
        return Err(format!("{name} exceeds its serialized-size bound"));
    }
    Ok(())
}

pub(crate) fn invalid<T>(message: &str) -> Result<T, String> {
    Err(message.to_owned())
}
