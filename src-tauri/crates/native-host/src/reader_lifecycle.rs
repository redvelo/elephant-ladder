use std::sync::{Arc, Weak, atomic::Ordering};

use arc_swap::ArcSwapOption;
use tokio::sync::{Mutex, MutexGuard, OwnedMutexGuard};

use super::{ActiveReaderSession, NavigationTicket};

#[derive(Clone, Default)]
pub struct ActiveReaderSessionView {
    current: Arc<ArcSwapOption<ActiveReaderSession>>,
    pub(super) network_activity: Arc<std::sync::Mutex<super::network_activity::BrowserActivity>>,
}

impl ActiveReaderSessionView {
    pub fn load(&self) -> Option<Arc<ActiveReaderSession>> {
        self.current.load_full()
    }
}

pub struct ReaderLifecycle {
    active: ActiveReaderSessionView,
    control: Arc<Mutex<ReaderLifecycleControl>>,
}

pub struct ReaderLifecycleControl {
    pub(super) navigation_ticket: Option<NavigationTicket>,
    active: ActiveReaderSessionView,
    import: Weak<()>,
}

#[derive(Clone, Copy)]
pub(super) enum ImportRefusal {
    /// A book is open, and its bytes must not move under it.
    Reading,
    /// Another import holds the slot, which it releases when it finishes.
    ImportActive,
}

impl ImportRefusal {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Reading => "publication import is unavailable while reading",
            Self::ImportActive => "another publication import is still finishing",
        }
    }
}

#[derive(Clone)]
pub(super) struct ReaderImportLease {
    _token: Arc<()>,
}

impl ReaderLifecycle {
    pub(super) fn new(
        active: ActiveReaderSessionView,
        navigation_ticket: Option<NavigationTicket>,
    ) -> Self {
        Self {
            active: active.clone(),
            control: Arc::new(Mutex::new(ReaderLifecycleControl {
                navigation_ticket,
                active,
                import: Weak::new(),
            })),
        }
    }

    pub fn active_view(&self) -> ActiveReaderSessionView {
        self.active.clone()
    }

    pub(super) fn load_active(&self) -> Option<Arc<ActiveReaderSession>> {
        self.active.load()
    }

    pub(super) async fn lock(&self) -> MutexGuard<'_, ReaderLifecycleControl> {
        self.control.lock().await
    }

    pub(super) async fn lock_owned(&self) -> OwnedMutexGuard<ReaderLifecycleControl> {
        self.control.clone().lock_owned().await
    }
}

impl ReaderLifecycleControl {
    /// Why an import cannot start, because the two reasons do not mean the same thing to the
    /// person waiting: one asks them to put a book down, the other passes on its own.
    pub(super) fn begin_import(&mut self) -> Result<ReaderImportLease, ImportRefusal> {
        if self.import.upgrade().is_some() {
            return Err(ImportRefusal::ImportActive);
        }
        if self
            .active
            .load()
            .is_some_and(|session| !session.revoked.load(Ordering::Acquire))
        {
            return Err(ImportRefusal::Reading);
        }
        let token = Arc::new(());
        self.import = Arc::downgrade(&token);
        Ok(ReaderImportLease { _token: token })
    }

    pub(super) fn import_active(&self) -> bool {
        self.import.strong_count() != 0
    }

    pub(super) fn reader_active(&self) -> bool {
        self.active.load().is_some()
    }

    pub(super) fn replace(&mut self, replacement: Arc<ActiveReaderSession>) {
        if let Some(current) = self.active.load() {
            current.revoked.store(true, Ordering::Release);
        }
        self.active.current.store(Some(replacement));
    }

    pub(super) fn clear_if(
        &mut self,
        predicate: impl FnOnce(&ActiveReaderSession) -> bool,
    ) -> ActiveSessionClear {
        let Some(current) = self.active.load() else {
            return ActiveSessionClear::Empty;
        };
        if !predicate(&current) {
            return ActiveSessionClear::Retained;
        }
        current.revoked.store(true, Ordering::Release);
        self.active.current.store(None);
        ActiveSessionClear::Cleared
    }
}

impl ReaderLifecycle {
    #[cfg(test)]
    pub(super) fn try_control(&self) -> tokio::sync::MutexGuard<'_, ReaderLifecycleControl> {
        self.control.try_lock().expect("reader lifecycle is busy")
    }

    #[cfg(test)]
    pub(super) fn control_slot(&self) -> Arc<Mutex<ReaderLifecycleControl>> {
        self.control.clone()
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ActiveSessionClear {
    Empty,
    Cleared,
    Retained,
}
