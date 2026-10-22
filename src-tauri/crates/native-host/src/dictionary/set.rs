//! What a dictionary installation is once it is open, and the capability a
//! reader session sees.

use super::valid_language;
use crate::Arc;
use crate::BTreeMap;
use crate::storage::{DictionaryInstallation, DictionaryPackType, DictionaryViewType};
use arc_swap::ArcSwap;
use elephant_ladder_dictionary_pack::{
    AudioCollection, DictionaryPack, DictionarySnapshot, DictionaryView, ExpectedPack, PackError,
    ProjectionOptions, RelationKind, Section,
};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use sha2::Sha256;

pub(crate) const MAX_DICTIONARY_REQUEST_BYTES: usize = 16 * 1024;
pub(crate) const MAX_REQUEST_ID_BYTES: usize = 128;
pub(crate) const MAX_HANDLE_BYTES: usize = 1024;
pub(crate) const AUDIO_MODIFICATION: &str = "Transcoded to Ogg Opus 24 kbps mono";

/// Summary facts for the first paint; the reader reads every entry's detail next.
pub(crate) const LOOKUP_PROJECTION: ProjectionOptions = ProjectionOptions::summary();

/// Section windows are paged; text and string lists are never cut.
pub(crate) const DETAIL_PROJECTION: ProjectionOptions = ProjectionOptions {
    text_bytes: usize::MAX,
    strings: usize::MAX,
    ..ProjectionOptions::detail()
};

/// The live set of installed dictionaries; reader sessions lease the current value.
pub(crate) struct DictionaryHost {
    pub(crate) current: ArcSwap<DictionarySet>,
}

impl DictionaryHost {
    pub(crate) fn new(set: DictionarySet) -> Self {
        Self {
            current: ArcSwap::from_pointee(set),
        }
    }

    pub(crate) fn lease(&self) -> Arc<DictionarySet> {
        self.current.load_full()
    }

    /// Publishes a rebuilt set. Existing reader sessions keep the set they leased.
    pub(crate) fn store(&self, set: DictionarySet) {
        self.current.store(Arc::new(set));
    }
}

/// Operations in flight, keyed by operation ID. One install runs at a time.
#[derive(Default)]
pub(crate) struct DictionaryOperations {
    pub(crate) active:
        std::sync::Mutex<std::collections::HashMap<String, Arc<super::acquire::OperationHandle>>>,
}

impl DictionaryOperations {
    pub(crate) fn insert(&self, handle: Arc<super::acquire::OperationHandle>) {
        self.active
            .lock()
            .expect("dictionary operations lock poisoned")
            .insert(handle.operation_id.clone(), handle);
    }

    pub(crate) fn get(&self, operation_id: &str) -> Option<Arc<super::acquire::OperationHandle>> {
        self.active
            .lock()
            .expect("dictionary operations lock poisoned")
            .get(operation_id)
            .cloned()
    }

    pub(crate) fn remove(&self, operation_id: &str) {
        self.active
            .lock()
            .expect("dictionary operations lock poisoned")
            .remove(operation_id);
    }

    pub(crate) fn active(&self) -> bool {
        !self
            .active
            .lock()
            .expect("dictionary operations lock poisoned")
            .is_empty()
    }
}

pub(crate) struct ViewDescriptor {
    pub(crate) view_id: String,
    pub(crate) view_type: DictionaryViewType,
    pub(crate) edition_language: Option<String>,
    pub(crate) source_language: String,
    pub(crate) target_language: Option<String>,
}

/// Opened dictionaries as of one registry state.
pub(crate) struct DictionarySet {
    pub(crate) revision: String,
    pub(crate) snapshot: DictionarySnapshot,
    pub(crate) views: Vec<ViewDescriptor>,
    pub(crate) audio: BTreeMap<String, AudioCollection>,
    pub(crate) unavailable: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DictionaryCapability {
    pub(crate) revision: String,
    pub(crate) views: Vec<DictionaryViewCapability>,
    pub(crate) audio_languages: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DictionaryViewCapability {
    pub(crate) view_id: String,
    pub(crate) view_type: &'static str,
    pub(crate) source_name: &'static str,
    pub(crate) edition_language: Option<String>,
    pub(crate) source_language: String,
    pub(crate) target_language: Option<String>,
}

impl DictionarySet {
    pub(crate) fn empty() -> Self {
        Self::from_parts(Vec::new(), Vec::new(), BTreeMap::new(), Vec::new())
            .expect("an empty dictionary set is valid")
    }

    fn from_parts(
        views: Vec<DictionaryView>,
        descriptors: Vec<ViewDescriptor>,
        audio: BTreeMap<String, AudioCollection>,
        unavailable: Vec<String>,
    ) -> Result<Self, String> {
        let snapshot = DictionarySnapshot::new(views).map_err(|error| error.to_string())?;
        let mut digest = Sha256::new();
        digest.update(snapshot.revision().as_bytes());
        for (language, collection) in &audio {
            digest.update((language.len() as u64).to_be_bytes());
            digest.update(language.as_bytes());
            digest.update(collection.pack_revision().as_bytes());
        }
        Ok(Self {
            revision: format!("{:x}", digest.finalize()),
            snapshot,
            views: descriptors,
            audio,
            unavailable,
        })
    }

    /// Opens every installed pack cheaply. Packs that fail to open are reported, not fatal.
    pub(crate) fn open(
        installations: &[DictionaryInstallation],
        records: &[crate::storage::DictionaryViewRecord],
    ) -> Self {
        let mut corpora = BTreeMap::new();
        let mut audio = BTreeMap::new();
        let mut unavailable = Vec::new();
        for installation in installations {
            let expected = match expected_pack(installation) {
                Ok(expected) => expected,
                Err(error) => {
                    unavailable.push(format!("{}: {error}", installation.pack_id));
                    continue;
                }
            };
            match installation.pack_type {
                DictionaryPackType::Corpus => {
                    match DictionaryPack::open_installed(&installation.directory, &expected) {
                        Ok(pack) => {
                            corpora.insert(installation.pack_id.clone(), Arc::new(pack));
                        }
                        Err(error) => {
                            unavailable.push(format!("{}: {error}", installation.pack_id))
                        }
                    }
                }
                DictionaryPackType::Audio => {
                    match AudioCollection::open_installed(&installation.directory, &expected) {
                        Ok(collection) => {
                            audio.insert(installation.corpus_language.clone(), collection);
                        }
                        Err(error) => {
                            unavailable.push(format!("{}: {error}", installation.pack_id))
                        }
                    }
                }
            }
        }
        let mut views = Vec::new();
        let mut descriptors = Vec::new();
        for record in records.iter().filter(|record| record.enabled) {
            let Some(pack) = corpora.get(&record.pack_id) else {
                continue;
            };
            let target = match (record.view_type, record.target_language.as_deref()) {
                (DictionaryViewType::Monolingual, None) => None,
                (DictionaryViewType::Bilingual, Some(target)) => Some(target),
                _ => continue,
            };
            let view = DictionaryView::new(record.view_id.clone(), pack.clone(), target);
            let Ok(view) = view else {
                continue;
            };
            views.push(view);
            descriptors.push(ViewDescriptor {
                view_id: record.view_id.clone(),
                view_type: record.view_type,
                edition_language: pack
                    .identity()
                    .wiktionary_edition
                    .strip_suffix("wiktionary")
                    .filter(|language| valid_language(language))
                    .map(str::to_owned),
                source_language: pack.corpus_language().to_owned(),
                target_language: record.target_language.clone(),
            });
        }
        Self::from_parts(views, descriptors, audio, unavailable.clone()).unwrap_or_else(|error| {
            tracing::warn!(event = "dictionary.unavailable", reason = %error);
            Self::empty()
        })
    }

    pub(crate) fn capability(&self) -> Option<DictionaryCapability> {
        if self.views.is_empty() {
            return None;
        }
        Some(DictionaryCapability {
            revision: self.revision.clone(),
            views: self
                .views
                .iter()
                .map(|view| DictionaryViewCapability {
                    view_id: view.view_id.clone(),
                    view_type: match view.view_type {
                        DictionaryViewType::Monolingual => "monolingual",
                        DictionaryViewType::Bilingual => "bilingual",
                    },
                    source_name: "Wiktionary",
                    edition_language: view.edition_language.clone(),
                    source_language: view.source_language.clone(),
                    target_language: view.target_language.clone(),
                })
                .collect(),
            audio_languages: self.audio.keys().cloned().collect(),
        })
    }

    pub(crate) fn unavailable(&self) -> &[String] {
        &self.unavailable
    }

    pub(crate) fn view_index(&self, view_id: &str) -> Option<usize> {
        self.snapshot
            .views()
            .iter()
            .position(|view| view.view_id() == view_id)
    }
}

pub(crate) fn expected_pack(installation: &DictionaryInstallation) -> Result<ExpectedPack, String> {
    Ok(ExpectedPack {
        pack_id: installation
            .pack_id
            .parse()
            .map_err(|error: PackError| error.to_string())?,
        pack_revision: installation.pack_revision.parse().map_err(str::to_owned)?,
        manifest_sha256: installation
            .manifest_sha256
            .parse()
            .map_err(str::to_owned)?,
    })
}

// Requests.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DictionaryLookupRequest {
    pub(crate) reader_session_id: String,
    pub(crate) revision: String,
    pub(crate) request_id: String,
    pub(crate) text: String,
    pub(crate) language: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DictionaryEntryRequest {
    pub(crate) reader_session_id: String,
    pub(crate) revision: String,
    pub(crate) entry: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DictionarySectionRequest {
    pub(crate) reader_session_id: String,
    pub(crate) revision: String,
    pub(crate) entry: String,
    pub(crate) section: SectionRequest,
    pub(crate) offset: usize,
    pub(crate) limit: usize,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum SectionRequest {
    Etymology,
    Pronunciations,
    Hyphenations,
    Forms,
    Senses,
    Translations,
    Relations { kind: RelationKind },
    Descendants,
    Notes,
    Categories,
    SenseExamples { sense: usize },
    SenseTranslations { sense: usize },
    SenseRelations { sense: usize, kind: RelationKind },
}

impl SectionRequest {
    pub(crate) const fn section(self) -> Section {
        match self {
            Self::Etymology => Section::Etymology,
            Self::Pronunciations => Section::Pronunciations,
            Self::Hyphenations => Section::Hyphenations,
            Self::Forms => Section::Forms,
            Self::Senses => Section::Senses,
            Self::Translations => Section::Translations,
            Self::Relations { kind } => Section::Relations(kind),
            Self::Descendants => Section::Descendants,
            Self::Notes => Section::Notes,
            Self::Categories => Section::Categories,
            Self::SenseExamples { sense } => Section::SenseExamples { sense },
            Self::SenseTranslations { sense } => Section::SenseTranslations { sense },
            Self::SenseRelations { sense, kind } => Section::SenseRelations { sense, kind },
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DictionaryAudioRequest {
    pub(crate) reader_session_id: String,
    pub(crate) revision: String,
    pub(crate) audio: String,
}
