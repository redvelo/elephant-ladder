//! Reading publication bytes out of a mounted snapshot.
//!
//! Unchanged resources up to a bounded size are read into memory on a blocking
//! worker, so the shared archive lock is released before the response body
//! drains. Larger resources stream through bounded channels where dropping the
//! body cancels production.

use super::{RangeDecision, parse_range, publication_document_headers};
use crate::http::{not_found, session_headers};
use crate::{
    ActiveReaderSession, MAX_BUFFERED_RESOURCE_BYTES, RESOURCE_STREAM_CHANNEL_CHUNKS,
    RESOURCE_STREAM_CHUNK_BYTES, ResponseRule, apply_reader_cover_cors_headers,
    duration_milliseconds, io, log_performance, storage,
};
use axum::{
    body::Body, body::Bytes, http::HeaderValue, http::StatusCode, http::header::ACCEPT_RANGES,
    http::header::CONTENT_LENGTH, http::header::CONTENT_RANGE, http::header::CONTENT_TYPE,
    response::IntoResponse, response::Response,
};
use epub_stack::{Epub, EpubPath, EpubZip};
use futures_util::stream;
use std::{
    fs::File, io::Read, sync::Arc, sync::atomic::AtomicBool, sync::atomic::Ordering, time::Instant,
};
use tokio::{sync::Semaphore, sync::mpsc};

pub async fn stream_imported_resource<R: io::Read + io::Seek + Send + 'static>(
    epub: Arc<Epub<EpubZip<R>>>,
    path: EpubPath,
    rule: &ResponseRule,
    head: bool,
    range: Option<&HeaderValue>,
    context: ResourceStreamContext,
) -> Response {
    let path_text = path.as_str().to_owned();
    if context
        .session
        .as_ref()
        .is_some_and(|session| session.revoked.load(Ordering::Acquire))
    {
        return not_found().await;
    }
    let total_bytes = context
        .sidecar
        .as_ref()
        .map_or(rule.size_bytes, |sidecar| sidecar.length);
    let mime = context
        .sidecar
        .as_ref()
        .and_then(|sidecar| sidecar.media_type.clone())
        .unwrap_or_else(|| rule.mime.clone());
    let Ok(total) = usize::try_from(total_bytes) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "resource size is unsupported",
        )
            .into_response();
    };
    // A HEAD response reports what the matching GET would return, including 206
    // and Content-Range, and still reads no resource bytes: the body is empty
    // and neither the buffered nor the streaming branch runs.
    let decision = parse_range(range, total);
    let (status, start, length, content_range) = match decision {
        RangeDecision::Full => (StatusCode::OK, 0, total, None),
        RangeDecision::Partial { start, end } => (
            StatusCode::PARTIAL_CONTENT,
            start,
            end - start + 1,
            Some(format!("bytes {start}-{end}/{total}")),
        ),
        RangeDecision::Unsatisfiable => {
            let mut response = (StatusCode::RANGE_NOT_SATISFIABLE, "").into_response();
            session_headers(response.headers_mut(), Some(&context.csp));
            response.headers_mut().insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{total}"))
                    .expect("content range must be valid"),
            );
            return response;
        }
    };
    let body = if head {
        Body::empty()
    } else if context.sidecar.is_none() && rule.size_bytes <= MAX_BUFFERED_RESOURCE_BYTES {
        let session = context.session.clone();
        let buffered_epub = epub.clone();
        let buffered_path = path.clone();
        let size_bytes = rule.size_bytes;
        let read_started = Instant::now();
        let entry = context
            .session
            .as_ref()
            .map(|session| session.entry.clone());
        // A cached entry needs neither the streaming permit nor a blocking worker.
        let cached = entry.as_ref().and_then(|entry| {
            entry
                .resources
                .lock()
                .expect("resource cache lock poisoned")
                .get(&path_text)
        });
        let hit = cached.is_some();
        let bytes = match cached {
            Some(bytes) => bytes,
            None => {
                let permit = match context.admission.acquire_owned().await {
                    Ok(permit) => permit,
                    Err(_) => {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            "resource streaming unavailable",
                        )
                            .into_response();
                    }
                };
                let read = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    if session
                        .as_ref()
                        .is_some_and(|session| session.revoked.load(Ordering::Acquire))
                    {
                        return Err(BufferedResourceError::Revoked);
                    }
                    read_resource_bytes(&buffered_epub, buffered_path, size_bytes)
                })
                .await;
                let bytes = match read {
                    Ok(Ok(bytes)) => bytes,
                    Ok(Err(BufferedResourceError::Revoked | BufferedResourceError::Missing)) => {
                        return not_found().await;
                    }
                    Ok(Err(BufferedResourceError::Unreadable)) => {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "resource is unreadable")
                            .into_response();
                    }
                    Err(_) => {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "resource worker failed")
                            .into_response();
                    }
                };
                if let Some(entry) = entry.as_ref() {
                    entry
                        .resources
                        .lock()
                        .expect("resource cache lock poisoned")
                        .insert(path_text.clone(), bytes.clone());
                }
                bytes
            }
        };
        if context
            .session
            .as_ref()
            .is_some_and(|session| session.revoked.load(Ordering::Acquire))
        {
            return not_found().await;
        }
        log_performance(serde_json::json!({
            "event": "publication_resource",
            "traceId": context.session.as_ref().map(|session| session.trace_id.as_ref()),
            "outcome": "ready",
            "buffered": true,
            "cache": if hit { "hit" } else { "miss" },
            "sourceBytes": size_bytes,
            "readMs": duration_milliseconds(read_started.elapsed()),
        }));
        Body::from(bytes.slice(start..start + length))
    } else {
        log_performance(serde_json::json!({
            "event": "publication_resource",
            "traceId": context.session.as_ref().map(|session| session.trace_id.as_ref()),
            "outcome": "ready",
            "buffered": false,
            "oversized": context.sidecar.is_none(),
            "sourceBytes": total_bytes,
        }));
        let permit = match context.admission.acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "resource streaming unavailable",
                )
                    .into_response();
            }
        };
        let (sender, receiver) = mpsc::channel(RESOURCE_STREAM_CHANNEL_CHUNKS);
        let session = context.session.clone();
        let sidecar = context.sidecar.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if session
                .as_ref()
                .is_some_and(|session| session.revoked.load(Ordering::Acquire))
            {
                return;
            }
            let revoked = session.as_ref().map(|session| &session.revoked);
            let result = match sidecar {
                Some(sidecar) => stream_sidecar_range(&sidecar, start, length, &sender, revoked),
                None => epub
                    .read_with(&path, |reader| {
                        stream_reader_range(reader, start, length, &sender, revoked)
                    })
                    .map_err(|error| io::Error::other(error.to_string()))
                    .and_then(|result| result),
            };
            if let Err(error) = result {
                let _ = sender.blocking_send(Err(error));
            }
        });
        Body::from_stream(stream::unfold(receiver, |mut receiver| async {
            receiver.recv().await.map(|item| (item, receiver))
        }))
    };
    let mut response = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, &mime)
        .header(CONTENT_LENGTH, length)
        .header(ACCEPT_RANGES, "bytes")
        .body(body)
        .expect("streaming publication response must be valid");
    session_headers(response.headers_mut(), Some(&context.csp));
    publication_document_headers(response.headers_mut(), &mime);
    if context.reader_cover_path.as_deref() == Some(path_text.as_str())
        && context
            .request_origin
            .as_ref()
            .and_then(|value| value.to_str().ok())
            == context.reader_cover_origin.as_deref()
    {
        apply_reader_cover_cors_headers(
            response.headers_mut(),
            context.reader_cover_origin.as_deref(),
        );
    }
    if let Some(content_range) = content_range {
        response.headers_mut().insert(
            CONTENT_RANGE,
            HeaderValue::from_str(&content_range).expect("content range must be valid"),
        );
    }
    response
}

pub struct ResourceStreamContext {
    pub csp: String,
    pub admission: Arc<Semaphore>,
    pub session: Option<Arc<ActiveReaderSession>>,
    pub request_origin: Option<HeaderValue>,
    pub reader_cover_path: Option<String>,
    pub reader_cover_origin: Option<Arc<str>>,
    pub sidecar: Option<storage::MediaSidecar>,
}

/// Reads at an absolute offset. Windows spells this `seek_read` and leaves the file pointer
/// after the bytes it read, where Unix leaves it alone; every read here carries its own offset,
/// so the difference does not reach the caller.
#[cfg(unix)]
fn read_at(file: &File, buffer: &mut [u8], position: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buffer, position)
}

#[cfg(windows)]
fn read_at(file: &File, buffer: &mut [u8], position: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buffer, position)
}

pub(crate) fn stream_sidecar_range(
    sidecar: &storage::MediaSidecar,
    start: usize,
    mut remaining: usize,
    sender: &mpsc::Sender<Result<Bytes, io::Error>>,
    revoked: Option<&AtomicBool>,
) -> io::Result<()> {
    let file = File::open(&sidecar.path)?;
    let mut position = u64::try_from(start)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "range start is unsupported"))?;
    let mut buffer = vec![0_u8; RESOURCE_STREAM_CHUNK_BYTES];
    while remaining > 0 {
        if revoked.is_some_and(|revoked| revoked.load(Ordering::Acquire)) {
            return Ok(());
        }
        let read = read_at(
            &file,
            &mut buffer[..remaining.min(RESOURCE_STREAM_CHUNK_BYTES)],
            position,
        )?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "media sidecar is shorter than its sealed length",
            ));
        }
        remaining -= read;
        position = position
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("media sidecar position overflowed"))?;
        if sender
            .blocking_send(Ok(Bytes::copy_from_slice(&buffer[..read])))
            .is_err()
        {
            return Ok(());
        }
    }
    Ok(())
}

pub(crate) enum BufferedResourceError {
    Revoked,
    Missing,
    Unreadable,
}

/// Reads one bounded entry so the archive lock is released before the response body drains.
pub(crate) fn read_resource_bytes<R: io::Read + io::Seek>(
    epub: &Epub<EpubZip<R>>,
    path: EpubPath,
    size_bytes: u64,
) -> Result<Bytes, BufferedResourceError> {
    let expected = usize::try_from(size_bytes).map_err(|_| BufferedResourceError::Unreadable)?;
    let bytes = epub
        .read_with(&path, |reader| {
            let mut bytes = Vec::with_capacity(expected);
            reader
                .take(size_bytes.saturating_add(1))
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        })
        .map_err(|_| BufferedResourceError::Missing)?
        .map_err(|_| BufferedResourceError::Unreadable)?;
    if bytes.len() != expected {
        return Err(BufferedResourceError::Unreadable);
    }
    Ok(Bytes::from(bytes))
}

pub(crate) fn stream_reader_range(
    reader: &mut dyn Read,
    mut skip: usize,
    mut remaining: usize,
    sender: &mpsc::Sender<Result<Bytes, io::Error>>,
    revoked: Option<&AtomicBool>,
) -> io::Result<()> {
    let mut buffer = vec![0_u8; RESOURCE_STREAM_CHUNK_BYTES];
    while skip > 0 {
        if revoked.is_some_and(|revoked| revoked.load(Ordering::Acquire)) {
            return Ok(());
        }
        let read = reader.read(&mut buffer[..skip.min(RESOURCE_STREAM_CHUNK_BYTES)])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "resource is shorter than metadata",
            ));
        }
        skip -= read;
    }
    while remaining > 0 {
        if revoked.is_some_and(|revoked| revoked.load(Ordering::Acquire)) {
            return Ok(());
        }
        let read = reader.read(&mut buffer[..remaining.min(RESOURCE_STREAM_CHUNK_BYTES)])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "resource is shorter than metadata",
            ));
        }
        remaining -= read;
        if sender
            .blocking_send(Ok(Bytes::copy_from_slice(&buffer[..read])))
            .is_err()
        {
            return Ok(());
        }
    }
    Ok(())
}
