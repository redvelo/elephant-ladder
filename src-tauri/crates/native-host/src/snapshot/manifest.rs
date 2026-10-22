//! What the host decided to serve for each resource, decided once at derivation.
//!
//! The manifest is persisted, so a reopened publication serves the same bytes with
//! the same headers without re-deriving. Each rule names the response a canonical
//! path gets, and a resource with no rule is not servable rather than served
//! guessed.

use std::collections::{BTreeMap, HashSet};

use epub_navigator::kernel::{DocumentAdapter, OccurrenceDocumentTarget, ReadingOrderOccurrence};
use epub_stack::resource::IdrefTarget;
use epub_stack::{EpubPath, PublicationAnalysis, analysis::AnalysisOutcome};
use serde::{Deserialize, Serialize};

use crate::mount::{content_type, measurement_dependency_mime};
use crate::{
    MAX_RESPONSE_MANIFEST_BYTES, MAX_RESPONSE_MIME_BYTES, MAX_RESPONSE_RESOURCES,
    MAX_SEARCH_STRING_BYTES,
};
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseManifest {
    pub resources: BTreeMap<String, ResponseRule>,
    /// Container entries the manifest does not declare but the publication links to.
    ///
    /// Withholding them is what the manifest asks for, and leaves a reader looking at a
    /// broken image or a dead link with nothing to explain it. Naming them here is how the
    /// application can say the book references files its own manifest omits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undeclared_references: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseRule {
    pub mime: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
    /// The container holds these bytes but the manifest does not list them, so they are not a
    /// publication resource and no plane serves them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub undeclared: bool,
    /// Serve another resource's bytes here: the first core media type in this resource's
    /// manifest fallback chain, for a foreign type the engine was never given a way to render.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_source: Option<String>,
    pub foreground: Foreground,
    pub foreground_preparation: ForegroundPreparationState,
    pub measurement: Measurement,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Foreground {
    Unchanged,
    Xhtml,
    Svg,
    FixedImage,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForegroundPreparationState {
    Eligible,
    Ineligible,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Measurement {
    InstrumentedXhtml,
    Unchanged,
    Denied,
}

pub(super) fn validate_response_manifest(manifest: &ResponseManifest) -> Result<(), String> {
    if manifest.resources.len() > MAX_RESPONSE_RESOURCES {
        return Err("response manifest exceeds its item bounds".to_owned());
    }
    let bytes = serde_json::to_vec(manifest)
        .map_err(|error| format!("could not validate response manifest: {error}"))?;
    if bytes.len() > MAX_RESPONSE_MANIFEST_BYTES {
        return Err("response manifest exceeds the 24 MiB storage bound".to_owned());
    }
    for (path, rule) in &manifest.resources {
        if path.is_empty()
            || path.len() > MAX_SEARCH_STRING_BYTES
            || EpubPath::new(path).is_err()
            || rule.mime.is_empty()
            || rule.mime.len() > MAX_RESPONSE_MIME_BYTES
        {
            return Err("response manifest contains an invalid resource rule".to_owned());
        }
        if matches!(rule.measurement, Measurement::InstrumentedXhtml)
            && !matches!(rule.foreground, Foreground::Xhtml)
        {
            return Err("response manifest contains an invalid measurement rule".to_owned());
        }
        if matches!(rule.measurement, Measurement::Unchanged)
            && !measurement_dependency_mime(&rule.mime)
        {
            return Err("response manifest contains an invalid measurement dependency".to_owned());
        }
        if let Some(source) = &rule.fallback_source
            && (source == path || EpubPath::new(source).is_err())
        {
            return Err("response manifest contains an invalid fallback source".to_owned());
        }
    }
    if let Some(source) = manifest
        .resources
        .values()
        .filter_map(|rule| rule.fallback_source.as_deref())
        .find(|source| !manifest.resources.contains_key(*source))
    {
        return Err(format!(
            "response manifest has no rule for fallback source: {source}"
        ));
    }
    Ok(())
}

pub fn response_manifest(
    analysis: &PublicationAnalysis,
    reading_order: &[ReadingOrderOccurrence],
) -> Result<ResponseManifest, String> {
    let effective_image_resources = reading_order
        .iter()
        .filter_map(|occurrence| match &occurrence.document {
            OccurrenceDocumentTarget::Loadable {
                resource,
                adapter: DocumentAdapter::FixedImage,
            } => Some(resource.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let effective_svg_resources = reading_order
        .iter()
        .filter_map(|occurrence| match &occurrence.document {
            OccurrenceDocumentTarget::Loadable {
                resource,
                adapter: DocumentAdapter::FixedSvg,
            } => Some(resource.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let package_path = analysis.resources().package_path().as_str();
    // A container entry no declaration names is not a publication resource. That only follows
    // when every declaration resolved: an href the manifest could not resolve, such as the
    // path-absolute ones OCF forbids but browsers still follow, leaves no way to tell which
    // entry it meant, so nothing is withheld.
    let manifest_resolved = analysis
        .resources()
        .declarations()
        .all(|declaration| declaration.target().resource().is_some());
    let mut decisions = BTreeMap::new();
    let mut undeclared_references = Vec::new();
    for facts in analysis.analyzed_resources() {
        let resource = facts.resource();
        let Some(path) = resource.local_path() else {
            continue;
        };
        let path = path.as_str();

        // Declaration fallback keeps host behavior deterministic if document analysis is incomplete.
        let xhtml = facts
            .content()
            .value()
            .is_some_and(|content| content.as_xhtml().is_some())
            || resource.has_xhtml_declaration();
        let svg = effective_svg_resources.contains(path)
            && (facts
                .content()
                .value()
                .is_some_and(|content| content.as_svg().is_some())
                || resource.has_svg_declaration());
        let executable_content_absent = matches!(
            facts.content(),
            AnalysisOutcome::Complete(content)
                if content.executable_content_detected() == Some(false)
        );
        let foreground_preparation = match epub_navigator::foreground_preparation_state(facts) {
            epub_navigator::ForegroundPreparationState::Eligible => {
                ForegroundPreparationState::Eligible
            }
            epub_navigator::ForegroundPreparationState::Ineligible => {
                ForegroundPreparationState::Ineligible
            }
            epub_navigator::ForegroundPreparationState::Unknown => {
                ForegroundPreparationState::Unknown
            }
        };
        let measurement_mime = if xhtml {
            "application/xhtml+xml"
        } else if resource.has_svg_declaration() {
            "image/svg+xml"
        } else if resource.has_stylesheet_declaration() {
            "text/css"
        } else {
            content_type(path)
        };
        let mime =
            if xhtml || resource.has_svg_declaration() || resource.has_stylesheet_declaration() {
                measurement_mime.to_owned()
            } else {
                agreed_declared_media_mime(resource).unwrap_or_else(|| measurement_mime.to_owned())
            };
        let foreign = declared_essence(resource).is_some_and(|essence| !core_media_type(&essence));
        let fallback = if foreign {
            core_media_fallback(analysis, resource)
        } else {
            None
        };
        let (mime, size_bytes) = match &fallback {
            Some(fallback) => (fallback.mime.clone(), Some(fallback.size_bytes)),
            None => (mime, resource.presence().size_bytes()),
        };
        if size_bytes.is_none() {
            tracing::warn!(event = "publication_resource_missing", path = %path);
        }
        // The package document is the one resource the manifest may not list.
        let undeclared =
            manifest_resolved && resource.declarations().len() == 0 && path != package_path;
        if undeclared && facts.incoming_references().next().is_some() {
            tracing::warn!(event = "publication_resource_undeclared", path = %path);
            undeclared_references.push(path.to_owned());
        }
        let rule = ResponseRule {
            mime,
            size_bytes: size_bytes.unwrap_or(0),
            missing: size_bytes.is_none(),
            fallback_source: fallback.map(|fallback| fallback.path),
            // The package document is the one resource the manifest may not list.
            undeclared,
            foreground: if effective_image_resources.contains(path) {
                Foreground::FixedImage
            } else if svg {
                Foreground::Svg
            } else if xhtml {
                Foreground::Xhtml
            } else {
                Foreground::Unchanged
            },
            foreground_preparation,
            measurement: if !svg
                && matches!(
                    facts.content(),
                    AnalysisOutcome::Complete(content) if content.as_xhtml().is_some()
                )
                && executable_content_absent
                && !resource.is_scripted()
            {
                Measurement::InstrumentedXhtml
            } else if measurement_dependency_mime(measurement_mime) {
                Measurement::Unchanged
            } else {
                Measurement::Denied
            },
        };
        if decisions.insert(path.to_owned(), rule).is_some() {
            return Err(format!("duplicate response resource path: {path}"));
        }
    }
    Ok(ResponseManifest {
        resources: decisions,
        undeclared_references,
    })
}

/// Returns the normalized authored audio or video type when every valid declaration agrees.
pub(crate) fn agreed_declared_media_mime(
    resource: epub_stack::resource::ResourceRef<'_>,
) -> Option<String> {
    let mut agreed: Option<String> = None;
    for declaration in resource.declarations() {
        let Some(media_type) = declaration.media_type().filter(|value| value.is_valid()) else {
            continue;
        };
        let normalized = normalized_media_mime(media_type)?;
        match &agreed {
            Some(existing) if *existing != normalized => return None,
            Some(_) => {}
            None => agreed = Some(normalized),
        }
    }
    agreed
}

pub(crate) fn normalized_media_mime(
    media_type: &epub_stack::resource::MediaType,
) -> Option<String> {
    let essence = media_type.essence()?.to_ascii_lowercase();
    if !(essence.starts_with("audio/") || essence.starts_with("video/")) {
        return None;
    }
    let mut parameters = Vec::new();
    for parameter in media_type.as_str().split(';').skip(1) {
        let (name, value) = parameter.trim().split_once('=')?;
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
        if name.is_empty() || value.is_empty() || value.contains(['"', '\\']) {
            return None;
        }
        parameters.push((name, value.to_owned()));
    }
    parameters.sort();
    Some(
        parameters
            .into_iter()
            .fold(essence, |mut mime, (name, value)| {
                mime.push_str(&format!("; {name}={value}"));
                mime
            }),
    )
}

pub(super) fn validate_effective_response_manifest(
    manifest: &ResponseManifest,
    reading_order: &[ReadingOrderOccurrence],
) -> Result<(), String> {
    for occurrence in reading_order {
        if let OccurrenceDocumentTarget::Loadable {
            resource,
            adapter: DocumentAdapter::FixedImage,
        } = &occurrence.document
            && !manifest
                .resources
                .get(resource)
                .is_some_and(|rule| matches!(rule.foreground, Foreground::FixedImage))
        {
            return Err(
                "response manifest does not provide an effective image document".to_owned(),
            );
        }
        if let OccurrenceDocumentTarget::Loadable {
            resource,
            adapter: DocumentAdapter::FixedSvg,
        } = &occurrence.document
            && !manifest
                .resources
                .get(resource)
                .is_some_and(|rule| matches!(rule.foreground, Foreground::Svg))
        {
            return Err(
                "response manifest does not instrument an effective SVG resource".to_owned(),
            );
        }
    }
    Ok(())
}

/// The manifest fallback chain is walked no further than this, as the media sidecars do.
const MAX_FALLBACK_DECLARATIONS: usize = 16;

/// A fallback this host will serve in a foreign resource's place.
struct CoreMediaFallback {
    path: String,
    mime: String,
    size_bytes: u64,
}

/// The media type every valid declaration of this resource agrees on.
fn declared_essence(resource: epub_stack::resource::ResourceRef<'_>) -> Option<String> {
    let mut agreed: Option<String> = None;
    for declaration in resource.declarations() {
        let Some(essence) = declaration
            .media_type()
            .filter(|value| value.is_valid())
            .and_then(epub_stack::resource::MediaType::essence)
        else {
            continue;
        };
        let essence = essence.to_ascii_lowercase();
        match &agreed {
            Some(existing) if *existing != essence => return None,
            Some(_) => {}
            None => agreed = Some(essence),
        }
    }
    agreed
}

/// Reports whether EPUB 3.4 lists this media type among the core media types, which a reading
/// system may be handed with no manifest fallback behind it.
fn core_media_type(essence: &str) -> bool {
    matches!(
        essence,
        "application/xhtml+xml"
            | "image/gif"
            | "image/jpeg"
            | "image/png"
            | "image/svg+xml"
            | "image/webp"
            | "image/avif"
            | "image/jxl"
            | "audio/mpeg"
            | "audio/mp4"
            | "audio/ogg"
            | "text/css"
            | "font/ttf"
            | "font/otf"
            | "font/woff"
            | "font/woff2"
            | "application/font-sfnt"
            | "application/font-woff"
            | "application/vnd.ms-opentype"
            | "application/x-font-ttf"
            | "application/javascript"
            | "application/ecmascript"
            | "text/javascript"
            | "application/x-dtbncx+xml"
            | "application/smil+xml"
    )
}

/// Walks a foreign resource's manifest fallback chain to the first core media type resource
/// whose bytes are present, which is what the reading engine is served in its place.
fn core_media_fallback(
    analysis: &PublicationAnalysis,
    resource: epub_stack::resource::ResourceRef<'_>,
) -> Option<CoreMediaFallback> {
    let index = analysis.resources();
    let mut pending: Vec<_> = resource
        .declarations()
        .filter_map(|declaration| declaration.fallback_target()?.declaration())
        .collect();
    let mut visited = HashSet::new();
    let mut hops = 0;
    while let Some(ordinal) = pending.pop() {
        hops += 1;
        if hops > MAX_FALLBACK_DECLARATIONS || !visited.insert(ordinal) {
            continue;
        }
        let Some(declaration) = index.declaration(ordinal) else {
            continue;
        };
        if let Some(next) = declaration
            .fallback_target()
            .and_then(IdrefTarget::declaration)
        {
            pending.push(next);
        }
        let Some(fallback) = declaration.resource() else {
            continue;
        };
        let (Some(path), Some(essence), Some(size_bytes)) = (
            fallback.local_path(),
            declared_essence(fallback),
            fallback.presence().size_bytes(),
        ) else {
            continue;
        };
        if !core_media_type(&essence) || path == resource.local_path()? {
            continue;
        }
        return Some(CoreMediaFallback {
            path: path.as_str().to_owned(),
            mime: essence,
            size_bytes,
        });
    }
    None
}
