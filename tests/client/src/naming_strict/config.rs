//! Settings for the Renamed properties client.
//!
//! swagger-rust writes this file once and never overwrites it: edit it
//! freely. Every generated request goes through it: [`url`] builds the
//! address, [`prepare`] can add headers (auth, app version) or change
//! anything else, [`unwrap_body`] unwraps response envelopes and
//! [`error_message`] reads error bodies.

use super::runtime::{ArrayFormat, HttpRequest, HttpResponse};
use serde_json::Value;
use std::time::Duration;

/// Where the API lives; endpoint paths (`/pets/{id}`) are appended to it.
pub const BASE_URL: &str = "https://naming.example";

/// How array query parameters are written (`Repeat`: `ids=1&ids=2`,
/// `Indices`: `ids[0]=1&ids[1]=2`, `Brackets`: `ids[]=1`, `Comma`: `ids=1,2`).
pub const ARRAY_FORMAT: ArrayFormat = ArrayFormat::Repeat;

/// The deadline for a whole request, response body included, for
/// transports that support one.
pub const TIMEOUT: Option<Duration> = Some(Duration::from_secs(30));

/// The address of a request for `path` (`/pets/42`), before the query
/// string.
#[must_use]
pub fn url(path: &str) -> String {
    format!("{}{}", BASE_URL.trim_end_matches('/'), path)
}

/// Called for every request right before it is sent.
pub fn prepare(request: &mut HttpRequest) {
    // request.set_header("Authorization", format!("Bearer {token}"));
    let _ = request;
}

/// The JSON a successful response is decoded from. Unwrap envelopes here,
/// e.g. `{"status": 200, "data": {…}}` → `{…}`.
#[must_use]
pub fn unwrap_body(body: Value) -> Value {
    body
}

/// The message of a failed (non-2xx) response, kept in `ApiError::Http`.
#[must_use]
pub fn error_message(response: &HttpResponse) -> Option<String> {
    let body: Value = serde_json::from_slice(&response.body).ok()?;
    ["message", "error", "detail", "title"]
        .iter()
        .find_map(|key| body.get(*key)?.as_str().map(str::to_owned))
}
