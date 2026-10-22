//! Process lifetime: preparing, running, and shutting down the host.

pub(crate) mod extension;
pub(crate) mod limits;
mod listener;
mod prepare;
mod run;
mod state;

pub(crate) use listener::bind_installation_origin_listeners;
pub use listener::{RunningServer, bind_listener, spawn_server};
pub use prepare::PreparedHost;
pub(crate) use prepare::open_dictionaries;
pub use run::RunningHost;
pub use state::ReaderState;
pub(crate) use state::SnapshotRegistrySlot;
