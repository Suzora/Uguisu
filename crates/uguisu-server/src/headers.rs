//! Response headers every answer carries (`docs/SECURITY.md` §3.6).
//!
//! One layer rather than a line in each handler, and it sets rather than adds,
//! so the byte routes' own `nosniff` is not duplicated.

use axum::http::{HeaderValue, header};
use axum::response::Response;

/// `Content-Security-Policy` for the SPA shell.
///
/// Everything that can execute comes from this origin: the built page loads one
/// external module and one stylesheet and contains no inline script or style, so
/// nothing here needs `unsafe-inline` or a hash. `media-src` is what lets an
/// archived episode play, and `connect-src` covers both `fetch` and the event
/// stream. `frame-ancestors` is the clickjacking control; `X-Frame-Options` is
/// not sent because it says less and browsers prefer this.
///
/// **`img-src` is the one open directive**, and deliberately. A podcast whose
/// artwork Uguisu has not fetched falls back to the URL its feed published
/// (`docs/WEB_UI.md`), and artwork fetching is off by default — so `'self'`
/// here would leave a default installation with no artwork at all. An image
/// cannot execute, and the privacy cost is the one the fallback already had.
const CSP: &str = "default-src 'self';                    img-src * data:;                    media-src 'self';                    connect-src 'self';                    font-src 'self';                    style-src 'self';                    script-src 'self';                    object-src 'none';                    base-uri 'none';                    form-action 'self';                    frame-ancestors 'none'";

/// Adds the headers that apply to everything, and the CSP to HTML.
pub(crate) async fn secure(mut response: Response) -> Response {
    let html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // `same-origin` rather than `no-referrer`: a link out of the UI should not
    // name which episode the reader was on, but a request Uguisu makes to
    // itself may say where it came from.
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    if html {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        );
    }
    response
}
