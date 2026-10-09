//! Extractors whose rejections answer the API's error envelope.
//!
//! axum's own `Json` and `Query` rejections are `text/plain` with no `error`
//! object and no `schema`, so a malformed body or an unparsable query
//! parameter used to leave the one documented shape (ADR 0038). These delegate
//! to axum and translate what comes back.
//!
//! An unmatched method is handled the same way, but by a router fallback
//! rather than a response-rewriting layer: a layer that turned every
//! non-JSON 4xx into an envelope would also eat the `416` the byte routes
//! answer with, which carries `Content-Range` and no body on purpose.

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::ApiError;

/// What a wrapped extractor answers with when it refuses a request.
pub(crate) type Rejection = (StatusCode, Json<ApiError>);

/// A required JSON body.
pub(crate) struct Body<T>(pub(crate) T);

/// An optional JSON body: absent is `None`, present and malformed is refused.
pub(crate) struct MaybeBody<T>(pub(crate) Option<T>);

/// Query parameters.
pub(crate) struct Params<T>(pub(crate) T);

impl<S, T> FromRequest<S> for Body<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Rejection;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(req, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|e| refused(&e))
    }
}

impl<S, T> FromRequest<S> for MaybeBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Rejection;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Option::<Json<T>>::from_request(req, state)
            .await
            .map(|body| Self(body.map(|Json(value)| value)))
            .map_err(|e| refused(&e))
    }
}

impl<S, T> FromRequestParts<S> for Params<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|e: QueryRejection| {
                ApiError::new(StatusCode::BAD_REQUEST, "invalid", e.body_text())
            })
    }
}

/// The router's answer to a path that exists under another method.
pub(crate) async fn method_not_allowed() -> Rejection {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "that path does not answer this method",
    )
}

/// axum decides the status; the kind follows from it, so a rejection variant
/// added by a future axum still lands in the envelope with a usable kind.
fn refused(e: &JsonRejection) -> Rejection {
    let status = e.status();
    ApiError::new(status, kind_for(status), e.body_text())
}

fn kind_for(status: StatusCode) -> &'static str {
    match status {
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
        StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
        StatusCode::INTERNAL_SERVER_ERROR => "internal",
        _ => "invalid",
    }
}
