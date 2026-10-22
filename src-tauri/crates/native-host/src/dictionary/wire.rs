//! The dictionary response wire format.
//!
//! Separates what a pack stores from what the reader receives: entry and audio
//! handles are opaque to the client, and pack-internal identifiers never appear
//! in a response.

use crate::dictionary::AUDIO_MODIFICATION;
use crate::dictionary::DictionarySet;
use crate::dictionary::RemoteRecording;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use elephant_ladder_dictionary_pack::{
    AudioDelivery, Bounded, Entry, EntryReference, Pronunciation, RecordingStatus,
};
use serde::Serialize;

// Wire values.

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireEntry {
    pub(crate) handle: String,
    pub(crate) headword: String,
    pub(crate) language_name: Option<String>,
    pub(crate) language_code: String,
    pub(crate) part_of_speech: Option<String>,
    pub(crate) part_of_speech_title: Option<String>,
    pub(crate) literal_meaning: Option<String>,
    pub(crate) etymology_number: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) raw_tags: Vec<String>,
    pub(crate) form_of: Vec<elephant_ladder_dictionary_pack::LemmaReference>,
    pub(crate) alt_of: Vec<elephant_ladder_dictionary_pack::LemmaReference>,
    pub(crate) classifiers: Vec<elephant_ladder_dictionary_pack::Classifier>,
    pub(crate) etymology: Bounded<String>,
    pub(crate) pronunciations: Bounded<WirePronunciation>,
    pub(crate) hyphenations: Bounded<elephant_ladder_dictionary_pack::Hyphenation>,
    pub(crate) forms: Bounded<elephant_ladder_dictionary_pack::Form>,
    pub(crate) senses: Bounded<elephant_ladder_dictionary_pack::Sense>,
    pub(crate) translations: Bounded<elephant_ladder_dictionary_pack::Translation>,
    pub(crate) sense_translation_total: usize,
    pub(crate) relations: Vec<elephant_ladder_dictionary_pack::RelationGroup>,
    pub(crate) descendants: Bounded<elephant_ladder_dictionary_pack::Descendant>,
    pub(crate) notes: Bounded<String>,
    pub(crate) categories: Bounded<String>,
    pub(crate) truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WirePronunciation {
    pub(crate) ipa: Option<String>,
    pub(crate) enpr: Option<String>,
    pub(crate) zh_pronunciation: Option<String>,
    pub(crate) hangeul: Option<String>,
    pub(crate) homophone: Option<String>,
    pub(crate) romanization: Option<String>,
    pub(crate) form: Option<String>,
    pub(crate) rhymes: Option<String>,
    pub(crate) text: Option<String>,
    pub(crate) note: Option<String>,
    pub(crate) other: Option<String>,
    pub(crate) sense: Option<String>,
    pub(crate) alternative: Option<String>,
    pub(crate) not_same_pronunciation: Option<bool>,
    pub(crate) audio_ipa: Option<String>,
    pub(crate) homophones: Vec<String>,
    pub(crate) hyphenation: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) raw_tags: Vec<String>,
    pub(crate) topics: Vec<String>,
    pub(crate) audio: Option<WireAudio>,
    pub(crate) truncated: bool,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub(crate) enum WireAudio {
    #[serde(rename_all = "camelCase")]
    Available {
        handle: String,
        duration_ms: u64,
        credit: WireCredit,
    },
    /// Playable by handle, but the host fetches the original from Commons to play it.
    Remote {
        handle: String,
        credit: WireCredit,
    },
    Unqualified,
    Missing,
    Failed,
    NotInCollection,
    CollectionNotInstalled,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireCredit {
    pub(crate) author: Option<String>,
    pub(crate) author_urls: Vec<String>,
    pub(crate) license: WireLicense,
    pub(crate) source_url: Option<String>,
    pub(crate) attribution_required: Option<bool>,
    pub(crate) modification: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireLicense {
    pub(crate) short_name: String,
    pub(crate) url: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum WireViewOutcome {
    #[serde(rename_all = "camelCase")]
    Completed {
        truncated: bool,
        entries: Vec<WireLookupMatch>,
    },
    NoTranslation {
        entries: Vec<WireLookupMatch>,
    },
    NoEntry,
    Failed,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireLookupMatch {
    pub(crate) match_class: elephant_ladder_dictionary_pack::MatchClass,
    pub(crate) entry: WireEntry,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireViewLookup {
    pub(crate) view_id: String,
    pub(crate) outcome: WireViewOutcome,
}

pub(crate) fn entry_handle(view_index: usize, reference: &EntryReference) -> String {
    format!(
        "e{view_index}.{}.{}",
        reference.entry_id.to_hex(),
        reference.selected_ordinal
    )
}

pub(crate) fn parse_entry_handle(
    set: &DictionarySet,
    handle: &str,
) -> Option<(usize, EntryReference)> {
    let mut parts = handle.strip_prefix('e')?.split('.');
    let view_index = parts.next()?.parse::<usize>().ok()?;
    let entry_hex = parts.next()?;
    let ordinal = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let entry_id = entry_hex.parse().ok()?;
    let pack = set.snapshot.views().get(view_index)?.pack();
    Some((
        view_index,
        EntryReference {
            pack_id: pack.pack_id().clone(),
            pack_revision: *pack.pack_revision(),
            entry_id,
            selected_ordinal: ordinal,
        },
    ))
}

pub(crate) fn audio_handle(language: &str, file_name: &str) -> String {
    format!("a.{language}.{}", URL_SAFE_NO_PAD.encode(file_name))
}

pub(crate) fn parse_audio_handle(handle: &str) -> Option<(String, String)> {
    let mut parts = handle.strip_prefix("a.")?.splitn(2, '.');
    let language = parts.next()?.to_owned();
    let file_name = String::from_utf8(URL_SAFE_NO_PAD.decode(parts.next()?).ok()?).ok()?;
    Some((language, file_name))
}

pub(crate) fn wire_entry(set: &DictionarySet, view_index: usize, entry: Entry) -> WireEntry {
    WireEntry {
        handle: entry_handle(view_index, &entry.reference),
        pronunciations: wire_pronunciations(set, &entry.language_code, entry.pronunciations),
        headword: entry.headword,
        language_name: entry.language_name,
        language_code: entry.language_code,
        part_of_speech: entry.part_of_speech,
        part_of_speech_title: entry.part_of_speech_title,
        literal_meaning: entry.literal_meaning,
        etymology_number: entry.etymology_number,
        tags: entry.tags,
        raw_tags: entry.raw_tags,
        form_of: entry.form_of,
        alt_of: entry.alt_of,
        classifiers: entry.classifiers,
        etymology: entry.etymology,
        hyphenations: entry.hyphenations,
        forms: entry.forms,
        senses: entry.senses,
        translations: entry.translations,
        sense_translation_total: entry.sense_translation_total,
        relations: entry.relations,
        descendants: entry.descendants,
        notes: entry.notes,
        categories: entry.categories,
        truncated: entry.truncated,
    }
}

pub(crate) fn wire_pronunciations(
    set: &DictionarySet,
    language: &str,
    pronunciations: Bounded<Pronunciation>,
) -> Bounded<WirePronunciation> {
    Bounded {
        offset: pronunciations.offset,
        total: pronunciations.total,
        items: pronunciations
            .items
            .into_iter()
            .map(|pronunciation| wire_pronunciation(set, language, pronunciation))
            .collect(),
    }
}

pub(crate) fn wire_pronunciation(
    set: &DictionarySet,
    language: &str,
    pronunciation: Pronunciation,
) -> WirePronunciation {
    WirePronunciation {
        audio: pronunciation
            .audio
            .as_deref()
            .map(|file_name| wire_audio(set, language, file_name)),
        ipa: pronunciation.ipa,
        enpr: pronunciation.enpr,
        zh_pronunciation: pronunciation.zh_pronunciation,
        hangeul: pronunciation.hangeul,
        homophone: pronunciation.homophone,
        romanization: pronunciation.romanization,
        form: pronunciation.form,
        rhymes: pronunciation.rhymes,
        text: pronunciation.text,
        note: pronunciation.note,
        other: pronunciation.other,
        sense: pronunciation.sense,
        alternative: pronunciation.alternative,
        not_same_pronunciation: pronunciation.not_same_pronunciation,
        audio_ipa: pronunciation.audio_ipa,
        homophones: pronunciation.homophones,
        hyphenation: pronunciation.hyphenation,
        tags: pronunciation.tags,
        raw_tags: pronunciation.raw_tags,
        topics: pronunciation.topics,
        truncated: pronunciation.truncated,
    }
}

pub(crate) fn wire_audio(set: &DictionarySet, language: &str, file_name: &str) -> WireAudio {
    let Some(collection) = set.audio.get(language) else {
        return WireAudio::CollectionNotInstalled;
    };
    let Ok(Some(recording)) = collection.recording(file_name) else {
        return WireAudio::NotInCollection;
    };
    let remote = collection.metadata().delivery == AudioDelivery::Remote
        && RemoteRecording::from_facts(&recording.facts).is_some();
    let facts = recording.facts;
    let credit = |license: elephant_ladder_dictionary_pack::RecordingLicense,
                  modification: Option<&'static str>| WireCredit {
        author: facts.author_text,
        author_urls: facts.author_urls,
        license: WireLicense {
            short_name: license.short_name,
            url: license.url,
        },
        source_url: facts.description_url,
        attribution_required: facts.attribution_required,
        modification,
    };
    match (facts.status, recording.audio, facts.license) {
        (RecordingStatus::Available, Some(audio), Some(license)) => WireAudio::Available {
            handle: audio_handle(language, file_name),
            duration_ms: audio.duration_ms,
            credit: credit(license, Some(AUDIO_MODIFICATION)),
        },
        // Played exactly as Commons serves it, so there is no modification to credit.
        (RecordingStatus::Available, None, Some(license)) if remote => WireAudio::Remote {
            handle: audio_handle(language, file_name),
            credit: credit(license, None),
        },
        (RecordingStatus::Unqualified, _, _) => WireAudio::Unqualified,
        (RecordingStatus::Missing, _, _) => WireAudio::Missing,
        _ => WireAudio::Failed,
    }
}

#[derive(Serialize)]
#[serde(tag = "type", content = "page", rename_all = "kebab-case")]
pub(crate) enum WireSection {
    Etymology(Bounded<String>),
    Pronunciations(Bounded<WirePronunciation>),
    Hyphenations(Bounded<elephant_ladder_dictionary_pack::Hyphenation>),
    Forms(Bounded<elephant_ladder_dictionary_pack::Form>),
    Senses(Bounded<elephant_ladder_dictionary_pack::Sense>),
    Translations(Bounded<elephant_ladder_dictionary_pack::Translation>),
    Relations(Bounded<elephant_ladder_dictionary_pack::Relation>),
    Descendants(Bounded<elephant_ladder_dictionary_pack::Descendant>),
    Notes(Bounded<String>),
    Categories(Bounded<String>),
    Examples(Bounded<elephant_ladder_dictionary_pack::Example>),
    NoSuchSense,
}
