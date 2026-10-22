use crate::ReaderState;
use crate::http::TrustedRead;
use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct PublicationDetails {
    description: Option<String>,
}

pub(super) async fn read(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    _: TrustedRead,
) -> Response {
    match state
        .storage
        .with("publication_description_load", move |storage| {
            storage.load_publication_description(&publication_id)
        })
        .await
    {
        Ok(Some(description)) => Json(PublicationDetails { description }).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "book details unavailable",
        )
            .into_response(),
    }
}
