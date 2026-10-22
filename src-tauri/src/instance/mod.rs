use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use elephant_ladder_native_host::MAX_ANNOTATION_LINK_BYTES;

// The single-instance handoff rides a Unix domain socket and is wired up only on Linux:
// macOS and Windows run without one for now. The fixture runner launches one process per
// fixture and deliberately skips handoff, so the socket is not built for it either.
#[cfg(all(target_os = "linux", not(feature = "fixture-runner")))]
pub mod socket;

/// A drop can carry a shelf, so publications are held more generously than links.
const MAX_QUEUED_PUBLICATIONS: usize = 64;

/// What the desktop handed this process: annotation links and publications to open.
/// Each kind is drained by its own reader, so one never consumes the other's arrivals.
#[derive(Clone, Default)]
pub struct LinkInbox(Arc<Mutex<Arrivals>>);

/// A publication this process was asked to take, whether reading it should remove it, and
/// whether the person asked to read it now or only to keep it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandedPublication {
    pub path: String,
    pub owned: bool,
    pub read: bool,
}

#[derive(Default)]
pub struct Arrivals {
    links: Vec<String>,
    publications: Vec<HandedPublication>,
}

impl LinkInbox {
    /// A publication dropped onto the window: the library keeps it without opening it, because
    /// a drop may carry a shelf and nobody asked to read all of it.
    pub fn push_dropped(&self, path: String) {
        self.push_publication(path, false, false);
    }

    fn push_publication(&self, path: String, owned: bool, read: bool) {
        let mut arrivals = self.0.lock().expect("link inbox lock poisoned");
        if arrivals.publications.len() < MAX_QUEUED_PUBLICATIONS {
            arrivals
                .publications
                .push(HandedPublication { path, owned, read });
        }
    }

    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().expect("link inbox lock poisoned").links)
    }

    /// Publications the desktop asked this process to open.
    pub fn take_publications(&self) -> Vec<HandedPublication> {
        std::mem::take(
            &mut self
                .0
                .lock()
                .expect("link inbox lock poisoned")
                .publications,
        )
    }
}

/// Accepts a readable local `.epub`, named as a path or a `file:` URL. Anything else, such as
/// a flag or a directory, is not an instruction this application understands.
fn opened_publication_path(argument: &str) -> Option<String> {
    let path = match argument.strip_prefix("file://") {
        Some(_) => url::Url::parse(argument).ok()?.to_file_path().ok()?,
        None if argument.starts_with('-') => return None,
        None => PathBuf::from(argument),
    };
    let path = path.canonicalize().ok()?;
    if !path.is_file()
        || !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("epub"))
    {
        return None;
    }
    let path = path.into_os_string().into_string().ok()?;
    (path.len() <= MAX_ANNOTATION_LINK_BYTES).then_some(path)
}

/// The publications among the paths dropped onto the window. A drop carries whatever the
/// person dragged, so anything that is not a readable `.epub` is simply not one of them.
pub fn dropped_publications<'a>(
    paths: impl IntoIterator<Item = &'a std::path::PathBuf>,
) -> Vec<String> {
    paths
        .into_iter()
        .filter_map(|path| opened_publication_path(&path.to_string_lossy()))
        .collect()
}
