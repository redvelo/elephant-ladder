//! The note a reference points at, read from the authored document.
//!
//! A reader that shows a note in place needs the note's own markup, and the only copy that is
//! certainly the publication's is the authored one the host holds. Taking it here keeps the
//! document itself inside its isolated frame: what leaves is one element's content, which the
//! reader sanitizes before it renders anything.

use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::Deserialize;
use std::sync::atomic::Ordering;

use crate::ReaderState;
use crate::http::TrustedRead;
use crate::http::auth::constant_time_equal;
use crate::session::valid_reader_session_id;
use epub_stack::EpubPath;

/// The authored document a note is read out of, bounded so one reference cannot ask the host to
/// hold a whole book in memory.
const MAX_NOTE_DOCUMENT_BYTES: u64 = 4 * 1024 * 1024;

/// Note routes on the trusted reader origin.
pub(crate) fn routes() -> Router<ReaderState> {
    Router::new().route(
        "/native/reader-sessions/{reader_session_id}/note",
        get(read_note),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NoteQuery {
    resource: String,
    fragment: String,
}

async fn read_note(
    State(state): State<ReaderState>,
    Path(reader_session_id): Path<String>,
    query: Result<Query<NoteQuery>, axum::extract::rejection::QueryRejection>,
    _: TrustedRead,
) -> Response {
    let Ok(Query(query)) = query else {
        return (StatusCode::BAD_REQUEST, "invalid note request").into_response();
    };
    if !valid_reader_session_id(&reader_session_id) {
        return (StatusCode::BAD_REQUEST, "invalid reader session").into_response();
    }
    let active_session = |state: &ReaderState| {
        state.lifecycle.load_active().filter(|session| {
            !session.revoked.load(Ordering::Acquire)
                && constant_time_equal(
                    session.reader_session_id.as_bytes(),
                    reader_session_id.as_bytes(),
                )
        })
    };
    let Some(session) = active_session(&state) else {
        return (StatusCode::CONFLICT, "reader session replaced").into_response();
    };
    let Ok(path) = EpubPath::new(&query.resource) else {
        return (StatusCode::BAD_REQUEST, "invalid note resource").into_response();
    };
    // Only a document this publication serves as XHTML holds a note; anything else is not a
    // place a reference can point, whatever the reader asked for.
    let servable = session
        .entry
        .responses
        .resources
        .get(query.resource.as_str())
        .filter(|rule| !rule.missing && !rule.undeclared)
        .filter(|rule| rule.mime.starts_with("application/xhtml+xml"));
    let Some(size_bytes) = servable.map(|rule| rule.size_bytes) else {
        return (StatusCode::NOT_FOUND, "no note there").into_response();
    };
    if size_bytes > MAX_NOTE_DOCUMENT_BYTES {
        return (StatusCode::NOT_FOUND, "no note there").into_response();
    }
    let fragment = query.fragment;
    let read = tokio::task::spawn_blocking(move || {
        if session.revoked.load(Ordering::Acquire) {
            return None;
        }
        let bytes = session
            .entry
            .epub
            .read_with(&path, |reader| {
                use std::io::Read as _;
                let mut bytes = Vec::new();
                reader
                    .take(MAX_NOTE_DOCUMENT_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes)
            })
            .ok()?
            .ok()?;
        authored_note(&bytes, &fragment)
    })
    .await;
    let Ok(Some(note)) = read else {
        return (StatusCode::NOT_FOUND, "no note there").into_response();
    };
    if active_session(&state).is_none() {
        return (StatusCode::CONFLICT, "reader session replaced").into_response();
    }
    axum::Json(serde_json::json!({
        "markup": note.markup,
        "direction": note.direction,
        "language": note.language,
    }))
    .into_response()
}

/// How much of one note the host will hand over. A note longer than this is not a note.
pub(crate) const MAX_NOTE_MARKUP_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthoredNote {
    /// The note element's content, exactly as authored.
    pub(crate) markup: String,
    /// The note element's own `dir`, which its content is read with.
    pub(crate) direction: Option<String>,
    /// The note element's own language, from `xml:lang` or `lang`.
    pub(crate) language: Option<String>,
}

/// How far out of an empty marker the note may be. A publication that anchors its note on an
/// empty element puts the note immediately around it, never a section away.
const MAX_NOTE_ENCLOSURE_STEPS: usize = 3;

/// Elements that hold a document rather than a note. Climbing out of one would hand over the
/// surrounding text instead of the note, so the climb stops before it.
const DOCUMENT_ENCLOSURES: [&str; 6] = ["body", "html", "section", "article", "nav", "main"];

/// Returns the content of the element the fragment names, or of the element enclosing it when
/// the fragment names a marker with nothing of its own.
///
/// Publications anchor a note either on the note itself or on an empty anchor at its head, and
/// both are equally common. The second says nothing by itself, so the note is what encloses it:
/// the nearest enclosing element that has content, within a few steps and never a sectioning
/// element, which holds a document rather than a note.
pub(crate) fn authored_note(source: &[u8], fragment: &str) -> Option<AuthoredNote> {
    if fragment.is_empty() {
        return None;
    }
    let text = std::str::from_utf8(source).ok()?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_end_names = false;
    let mut buffer = Vec::new();
    let mut open: Vec<Opened> = Vec::new();
    let mut opened_count = 0u64;
    // The element the fragment names, and then each element enclosing it in turn. An element
    // opened after it is a sibling or a descendant, which its larger ordinal tells apart.
    let mut named: Option<u64> = None;
    // The named element's own text, to tell a note from the marker at its head.
    let mut named_depth: Option<usize> = None;
    let mut named_text = String::new();
    let mut climbing = false;
    let mut steps = 0usize;
    let mut last_text_end = 0usize;
    loop {
        let position = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buffer) {
            Err(_) => return None,
            Ok(Event::Eof) => return None,
            Ok(Event::Start(element)) => {
                opened_count += 1;
                if named.is_none() && names_fragment(&element, fragment) {
                    named = Some(opened_count);
                    named_depth = Some(open.len());
                }
                open.push(opened(
                    &element,
                    opened_count,
                    reader.buffer_position() as usize,
                ));
            }
            Ok(Event::Empty(element)) => {
                // The marker carries nothing itself, so the note is whatever encloses it.
                if named.is_none() && names_fragment(&element, fragment) {
                    named = Some(open.last()?.ordinal);
                    climbing = true;
                }
            }
            Ok(Event::Text(content)) => {
                let content: &str = content.as_ref();
                if content.chars().any(|character| !character.is_whitespace()) {
                    last_text_end = reader.buffer_position() as usize;
                }
                if named_depth.is_some_and(|depth| open.len() > depth) {
                    named_text.push_str(content);
                }
            }
            Ok(Event::End(_)) => {
                let enclosing = open.pop()?;
                let Some(named) = named else { continue };
                if enclosing.ordinal > named {
                    continue;
                }
                // What the fragment names is handed over whatever it is; what merely encloses it
                // is handed over only while it is still a note and not the document around it.
                if climbing && DOCUMENT_ENCLOSURES.contains(&enclosing.name.as_str()) {
                    return None;
                }
                // Content is text the reader would see, not markup: an element holding nothing
                // but an empty marker has as little to show as the marker itself. Nor does a link
                // that is only a marker, which is the note's way back to the text rather than the
                // note: publications put the identifier on that as readily as on an empty anchor.
                let marker_link = enclosing.ordinal == named
                    && enclosing.name == "a"
                    && is_marker(named_text.trim());
                if last_text_end > enclosing.content_start && !marker_link {
                    let markup = text.get(enclosing.content_start..position)?;
                    if markup.len() > MAX_NOTE_MARKUP_BYTES {
                        return None;
                    }
                    return Some(AuthoredNote {
                        markup: markup.to_owned(),
                        direction: enclosing.direction,
                        language: enclosing.language,
                    });
                }
                if steps >= MAX_NOTE_ENCLOSURE_STEPS {
                    return None;
                }
                steps += 1;
                climbing = true;
            }
            Ok(_) => {}
        }
        buffer.clear();
    }
}

/// A number, a symbol, or a bracketed few characters: what a publication prints as a reference,
/// and never a note.
fn is_marker(text: &str) -> bool {
    !text.is_empty() && text.chars().count() <= 8 && !text.chars().any(char::is_whitespace)
}

struct Opened {
    name: String,
    ordinal: u64,
    content_start: usize,
    direction: Option<String>,
    language: Option<String>,
}

fn opened(
    element: &quick_xml::events::BytesStart<'_>,
    ordinal: u64,
    content_start: usize,
) -> Opened {
    Opened {
        name: element.local_name().into_inner().to_owned(),
        ordinal,
        content_start,
        direction: attribute(element, "dir"),
        language: attribute(element, "xml:lang").or_else(|| attribute(element, "lang")),
    }
}

fn names_fragment(element: &quick_xml::events::BytesStart<'_>, fragment: &str) -> bool {
    ["id", "xml:id"]
        .into_iter()
        .any(|name| attribute(element, name).as_deref() == Some(fragment))
}

/// An identifier, a direction, and a language tag are all plain tokens, so the authored bytes
/// are the value. A document that escapes one simply does not match, which is the safe way for
/// this to be wrong.
fn attribute(element: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    element.attributes().flatten().find_map(|attribute| {
        (attribute.key.as_ref().as_bytes() == name.as_bytes())
            .then(|| attribute.value.as_ref().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT: &[u8] = br##"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
  <body>
    <p>Text with <a epub:type="noteref" href="#n1">a reference</a>.</p>
    <aside epub:type="footnote" id="n1" dir="rtl" xml:lang="he">Note <em>one</em>, with <a href="#back">a way back</a>.</aside>
    <aside epub:type="footnote" id="n2"><p>Note two.</p></aside>
    <aside id="empty"/>
    <div id="outer"><span id="n1-inner">nested</span></div>
  </body>
</html>"##;

    #[test]
    fn takes_the_note_the_fragment_names_with_the_direction_it_is_read_with() {
        let note = authored_note(DOCUMENT, "n1").unwrap();
        assert_eq!(
            note.markup,
            r##"Note <em>one</em>, with <a href="#back">a way back</a>."##
        );
        assert_eq!(note.direction.as_deref(), Some("rtl"));
        assert_eq!(note.language.as_deref(), Some("he"));
    }

    #[test]
    fn keeps_a_block_note_whole_and_states_no_direction_the_note_does_not() {
        let note = authored_note(DOCUMENT, "n2").unwrap();
        assert_eq!(note.markup, "<p>Note two.</p>");
        assert_eq!(note.direction, None);
        assert_eq!(note.language, None);
    }

    #[test]
    fn has_nothing_to_show_for_a_fragment_that_is_absent_empty_or_unnamed() {
        assert_eq!(authored_note(DOCUMENT, "missing"), None);
        assert_eq!(authored_note(DOCUMENT, "empty"), None);
        assert_eq!(authored_note(DOCUMENT, ""), None);
        assert_eq!(authored_note(b"<html", "n1"), None);
    }

    #[test]
    fn takes_the_element_that_names_the_fragment_rather_than_one_that_contains_it() {
        assert_eq!(
            authored_note(DOCUMENT, "n1-inner").unwrap().markup,
            "nested"
        );
        assert_eq!(
            authored_note(DOCUMENT, "outer").unwrap().markup,
            r#"<span id="n1-inner">nested</span>"#
        );
    }

    /// The shapes Penguin's Capital and a calibre endnote build actually use: the note itself
    /// carries the identifier, or an empty marker at its head does.
    const ENCLOSED: &[u8] = br##"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <body>
    <p class="footnote" id="named"><a href="chapter001.xhtml#ref">1.</a> Karl Marx, Zur Kritik.</p>
    <p class="notes"><a id="emptied"></a><a href="part0002.html#ref">2.</a> Engels to Lavrov.</p>
    <p class="notes"><sup><a id="selfclosed"/></sup> A note behind a raised marker.</p>
    <p class="notes"><a href="12_Chapter_1.html#enref1" id="backlinked">1.</a> Kroeber did not define peasants.</p>
    <p class="notes"><a id="silent"></a><em><span/></em></p>
    <span id="loose"></span>
    <div><div><div><div><a id="buried"></a></div></div></div></div> Too far out to be a note.
  </body>
</html>"##;

    #[test]
    fn takes_the_note_around_a_marker_that_carries_nothing_itself() {
        assert_eq!(
            authored_note(ENCLOSED, "emptied").unwrap().markup,
            r##"<a id="emptied"></a><a href="part0002.html#ref">2.</a> Engels to Lavrov."##
        );
        assert_eq!(
            authored_note(ENCLOSED, "selfclosed").unwrap().markup,
            r##"<sup><a id="selfclosed"/></sup> A note behind a raised marker."##
        );
    }

    #[test]
    fn takes_the_note_around_a_marker_that_is_only_the_way_back() {
        assert_eq!(
            authored_note(ENCLOSED, "backlinked").unwrap().markup,
            r##"<a href="12_Chapter_1.html#enref1" id="backlinked">1.</a> Kroeber did not define peasants."##
        );
    }

    #[test]
    fn still_takes_the_note_that_carries_the_identifier_itself() {
        assert_eq!(
            authored_note(ENCLOSED, "named").unwrap().markup,
            r##"<a href="chapter001.xhtml#ref">1.</a> Karl Marx, Zur Kritik."##
        );
    }

    #[test]
    fn climbs_out_of_a_marker_no_further_than_a_note_reaches() {
        // Nothing encloses these but the document, which is not a note however little the
        // marker says.
        assert_eq!(authored_note(ENCLOSED, "silent"), None);
        assert_eq!(authored_note(ENCLOSED, "loose"), None);
        assert_eq!(authored_note(ENCLOSED, "buried"), None);
    }

    #[test]
    fn refuses_a_note_longer_than_a_note() {
        let long = format!(
            r#"<html><body><aside id="n">{}</aside></body></html>"#,
            "x".repeat(MAX_NOTE_MARKUP_BYTES + 1)
        );
        assert_eq!(authored_note(long.as_bytes(), "n"), None);
    }
}
