pub(crate) mod acquire;
pub(super) mod catalog;
mod manage;
pub(super) mod remote_audio;
mod serve;
mod set;
mod wire;

use remote_audio::RemoteRecording;

#[cfg(debug_assertions)]
pub(crate) use manage::install_development_packs;
pub(crate) use manage::{
    cancel_operation, catalog_listing, install, installations, operation, operations,
    resume_interrupted_installs, uninstall,
};
pub(crate) use serve::{
    DictionaryInstallRequest, completed, dictionary_unavailable, routes, valid_language,
};
#[cfg(debug_assertions)]
pub(crate) use set::expected_pack;
pub(crate) use set::{
    AUDIO_MODIFICATION, DETAIL_PROJECTION, DictionaryAudioRequest, DictionaryCapability,
    DictionaryEntryRequest, DictionaryHost, DictionaryLookupRequest, DictionaryOperations,
    DictionarySectionRequest, DictionarySet, LOOKUP_PROJECTION, MAX_DICTIONARY_REQUEST_BYTES,
    MAX_HANDLE_BYTES, MAX_REQUEST_ID_BYTES,
};
