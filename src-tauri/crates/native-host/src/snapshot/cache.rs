//! Byte-bounded caches over one mounted publication's resources.
//!
//! Both caches are sized in bytes rather than entries, because a publication's
//! resources differ by orders of magnitude and an entry count would not bound
//! memory. Eviction is least-recently-used within the mounted snapshot, and a
//! cache is dropped whole when the publication is unmounted.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;

use crate::mount::{PublicationResponseProfile, ResponseRepresentation};
use crate::{
    RESOURCE_CACHE_BYTES, RESOURCE_CACHE_ENTRIES, TRANSFORM_CACHE_BYTES, TRANSFORM_CACHE_ENTRIES,
};

#[derive(Default)]
pub struct TransformCache {
    pub entries: HashMap<TransformCacheKey, TransformCacheEntry>,
    pub bytes: usize,
    pub clock: u64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TransformCacheKey {
    pub path: String,
    pub(crate) representation: ResponseRepresentation,
    pub profile: PublicationResponseProfile,
    pub nested_publication_base_url: Option<Arc<str>>,
    pub content_script_nonce: Option<Arc<str>>,
}

pub struct TransformCacheEntry {
    pub(crate) bytes: Bytes,
    pub(crate) used: u64,
}

pub(crate) struct CacheInsertOutcome {
    pub(crate) bytes: Bytes,
    pub(crate) cached: bool,
    pub(crate) evictions: usize,
}

pub(crate) struct TransformedOutcome {
    pub(crate) bytes: Bytes,
    pub(crate) hit: bool,
    pub(crate) cached: bool,
    pub(crate) evictions: usize,
    pub(crate) source_bytes: Option<usize>,
    pub(crate) resource_read_ms: Option<u64>,
    pub(crate) transform_ms: Option<u64>,
}

#[derive(Default)]
pub struct ResourceCache {
    pub(crate) entries: HashMap<String, ResourceCacheEntry>,
    pub(crate) bytes: usize,
    pub(crate) clock: u64,
}

pub(crate) struct ResourceCacheEntry {
    pub(crate) bytes: Bytes,
    pub(crate) used: u64,
}

impl TransformCache {
    pub(crate) fn get(&mut self, key: &TransformCacheKey) -> Option<Bytes> {
        self.clock = self.clock.wrapping_add(1);
        let entry = self.entries.get_mut(key)?;
        entry.used = self.clock;
        Some(entry.bytes.clone())
    }

    pub(crate) fn insert(&mut self, key: TransformCacheKey, bytes: Bytes) -> CacheInsertOutcome {
        if bytes.len() > TRANSFORM_CACHE_BYTES {
            return CacheInsertOutcome {
                bytes,
                cached: false,
                evictions: 0,
            };
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.bytes -= previous.bytes.len();
        }
        self.clock = self.clock.wrapping_add(1);
        let mut evictions = 0;
        while self.entries.len() >= TRANSFORM_CACHE_ENTRIES
            || self.bytes.saturating_add(bytes.len()) > TRANSFORM_CACHE_BYTES
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.bytes -= removed.bytes.len();
                evictions += 1;
            }
        }
        self.bytes += bytes.len();
        self.entries.insert(
            key,
            TransformCacheEntry {
                bytes: bytes.clone(),
                used: self.clock,
            },
        );
        CacheInsertOutcome {
            bytes,
            cached: true,
            evictions,
        }
    }
}

impl ResourceCache {
    pub(crate) fn get(&mut self, path: &str) -> Option<Bytes> {
        self.clock = self.clock.wrapping_add(1);
        let entry = self.entries.get_mut(path)?;
        entry.used = self.clock;
        Some(entry.bytes.clone())
    }

    pub(crate) fn insert(&mut self, path: String, bytes: Bytes) -> usize {
        if bytes.len() > RESOURCE_CACHE_BYTES {
            return 0;
        }
        if let Some(previous) = self.entries.remove(&path) {
            self.bytes -= previous.bytes.len();
        }
        self.clock = self.clock.wrapping_add(1);
        let mut evictions = 0;
        while self.entries.len() >= RESOURCE_CACHE_ENTRIES
            || self.bytes.saturating_add(bytes.len()) > RESOURCE_CACHE_BYTES
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.bytes -= removed.bytes.len();
                evictions += 1;
            }
        }
        self.bytes += bytes.len();
        self.entries.insert(
            path,
            ResourceCacheEntry {
                bytes,
                used: self.clock,
            },
        );
        evictions
    }
}
