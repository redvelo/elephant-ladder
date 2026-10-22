//! Cover images: which resource a publication offers as its cover, staging its
//! bytes beside the archive, and the headers that let the reader page display it.
//!
//! A cover is copied out at import so the library can list a publication without
//! opening its archive, and it is served from the reader origin under explicit
//! cross-origin headers rather than by relaxing the publication's own.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path as FsPath;

use axum::http::{HeaderMap, HeaderValue, header::ORIGIN};
use axum::response::Response;
use epub_stack::{EpubPath, resource::ResourceSelection};
use serde_json::Value;

use crate::snapshot::{Foreground, ImportedEpub, ResponseRule};
use crate::staging::StagedFile;
use crate::storage::is_epub_core_image_mime;
use crate::{io, random_id};

pub(crate) struct StagedCover {
    pub(crate) guard: StagedFile,
    pub(crate) hash: String,
    pub(crate) size: u64,
    pub(crate) mime: String,
}

pub(crate) fn stage_selected_cover(
    epub: &ImportedEpub,
    facts: &epub_navigator::facts::PublicationFacts<'_>,
    staging: &FsPath,
) -> Result<Option<StagedCover>, String> {
    let declaration = match &facts.resources.selections().cover {
        ResourceSelection::Selected {
            declaration: Some(declaration),
            resource: Some(_),
            ..
        } => *declaration,
        _ => return Ok(None),
    };
    let declaration = epub
        .resources()
        .declaration(declaration)
        .ok_or("selected cover declaration is invalid")?;
    let Some(mime) = declaration
        .media_type()
        .and_then(|media_type| media_type.essence())
        .filter(|mime| is_epub_core_image_mime(mime))
        .map(str::to_owned)
    else {
        return Ok(None);
    };
    let cover_path = epub
        .resources()
        .cover_image()
        .and_then(|resource| resource.local_path())
        .ok_or("selected cover resource is unavailable")?
        .clone();
    let (mut file, guard) = create_cover_staging_file(staging)?;
    let (hash, size) = epub
        .read_with(&cover_path, |reader| copy_cover_resource(reader, &mut file))
        .map_err(|error| format!("could not open selected cover resource: {error}"))?
        .map_err(|error| format!("could not stage selected cover resource: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("could not sync selected cover resource: {error}"))?;
    drop(file);
    Ok(Some(StagedCover {
        guard,
        hash,
        size,
        mime,
    }))
}

fn create_cover_staging_file(staging: &FsPath) -> Result<(File, StagedFile), String> {
    for _ in 0..16 {
        let path = staging.join(format!(
            "{}.cover.part",
            random_id(16).map_err(|error| error.to_string())?
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, StagedFile::new(path))),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("could not create cover staging file: {error}")),
        }
    }
    Err("could not allocate cover staging file".to_owned())
}

fn copy_cover_resource(reader: &mut dyn Read, writer: &mut File) -> io::Result<(String, u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        size = size
            .checked_add(u64::try_from(read).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other("cover resource size overflow"))?;
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

pub(crate) fn selected_cover_path(facts: &Value) -> Option<String> {
    facts
        .get("resources")?
        .get("coverPath")?
        .as_str()
        .map(str::to_owned)
}

pub(crate) fn apply_reader_cover_cors(
    response: &mut Response,
    path: &EpubPath,
    request_headers: &HeaderMap,
    reader_origin: &str,
    facts: &Value,
) {
    if selected_cover_path(facts).as_deref() != Some(path.as_str())
        || request_headers
            .get(ORIGIN)
            .and_then(|value| value.to_str().ok())
            != Some(reader_origin)
    {
        return;
    }
    apply_reader_cover_cors_headers(response.headers_mut(), Some(reader_origin));
}

/// The reader paints an entering roll page's own bytes while its frame is still uncomposited,
/// so a spine image is readable by the reader origin exactly as a cover is.
pub(crate) fn apply_reader_page_image_cors(
    response: &mut Response,
    rule: &ResponseRule,
    request_headers: &HeaderMap,
    reader_origin: &str,
) {
    if !matches!(rule.foreground, Foreground::FixedImage)
        || request_headers
            .get(ORIGIN)
            .and_then(|value| value.to_str().ok())
            != Some(reader_origin)
    {
        return;
    }
    apply_reader_cover_cors_headers(response.headers_mut(), Some(reader_origin));
}

pub(crate) fn apply_reader_cover_cors_headers(headers: &mut HeaderMap, origin: Option<&str>) {
    let Some(origin) = origin else { return };
    headers.insert(
        "access-control-allow-origin",
        HeaderValue::from_str(origin).expect("reader origin must be valid"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("cross-origin"),
    );
    headers.insert("vary", HeaderValue::from_static("Origin"));
}
