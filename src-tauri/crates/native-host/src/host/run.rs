//! A host that is serving, and how it stops.

use std::time::Duration;

use crate::network_activity::NetworkActivityObserver;
use crate::storage;
use crate::{NavigationRecovery, RunningServer};

pub struct RunningHost {
    pub servers: Vec<RunningServer>,
    pub runtime: Option<tokio::runtime::Runtime>,
    pub storage: Option<storage::StorageHandle>,
    pub navigation_recovery: Option<NavigationRecovery>,
}

impl RunningHost {
    pub fn navigation_recovery(&self) -> NavigationRecovery {
        self.navigation_recovery
            .clone()
            .expect("running native host must have navigation recovery")
    }

    pub fn stop(mut self) {
        if let Some(recovery) = &self.navigation_recovery {
            NetworkActivityObserver::new(recovery.lifecycle.active_view()).shutdown();
        }
        for server in &mut self.servers {
            if let Some(shutdown) = server.shutdown.take() {
                let _ = shutdown.send(());
            }
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
        if let Some(storage) = self.storage.take() {
            storage
                .lock()
                .expect("native storage lock poisoned")
                .checkpoint();
        }
    }
}
