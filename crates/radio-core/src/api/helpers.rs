//! What the handlers share: the answers, the errors and the reading of what is asked. Task 4.1 of docs/rust-port.md,
//! ported from `_count`, `_json_body`, `_parse_*` and the `web.HTTP*` errors of `dj.py`.

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::Value;

/// An error answer: a status and a line of text, as aiohttp's `web.HTTPBadRequest(text=...)` and the rest make them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub text: String,
}

impl ApiError {
    pub fn new(status: StatusCode, text: impl Into<String>) -> Self {
        Self {
            status,
            text: text.into(),
        }
    }

    pub fn bad_request(text: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, text)
    }

    pub fn not_found(text: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, text)
    }

    pub fn conflict(text: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, text)
    }

    pub fn unavailable(text: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, text)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = Response::new(Body::from(self.text));
        *response.status_mut() = self.status;
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        response
    }
}

/// A JSON answer (`web.json_response`).
pub fn json_response(status: StatusCode, value: &Value) -> Response {
    let mut response = Response::new(Body::from(
        serde_json::to_vec(value).expect("a value is JSON"),
    ));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

pub fn ok(value: &Value) -> Response {
    json_response(StatusCode::OK, value)
}

/// Python's `repr()` of a string.
pub fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python's `repr()` of a list of strings: `['a', 'b']`.
pub fn py_list_repr<S: AsRef<str>>(items: &[S]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|s| py_repr(s.as_ref()))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Python's `int(text)`: spaces around, a sign, digits with single underscores between them.
pub fn python_int(text: &str) -> Option<i64> {
    let text = text.trim();
    let (negative, digits) = match text.as_bytes().first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let value: i64 = digits.replace('_', "").parse().ok()?;
    Some(if negative { -value } else { value })
}

/// The first value of a name in the query string.
pub fn query_value<'a>(query: &'a [(String, String)], name: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// `_count`: how many tracks to list, from the query.
pub fn count(query: &[(String, String)], name: &str, default: usize) -> Result<usize, ApiError> {
    let Some(text) = query_value(query, name) else {
        return Ok(default.min(crate::radio::MAX_LISTED));
    };
    let Some(value) = python_int(text) else {
        return Err(ApiError::bad_request(format!("{name} must be a number")));
    };
    if value < 0 {
        return Err(ApiError::bad_request(format!(
            "{name} must not be negative"
        )));
    }
    Ok((value as usize).min(crate::radio::MAX_LISTED))
}

/// `_json_body`
pub fn json_body(body: &[u8]) -> Result<Value, ApiError> {
    serde_json::from_slice(body).map_err(|_| ApiError::bad_request("body must be JSON"))
}

fn strings(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// `_parse_moods`
pub fn parse_moods(body: &Value) -> Result<Vec<String>, ApiError> {
    let Some(moods) = body.get("moods").and_then(strings) else {
        return Err(ApiError::bad_request("expected {\"moods\": [...]}"));
    };
    let mut unknown: Vec<&String> = moods
        .iter()
        .filter(|m| !crate::mix::choose::ALL_MOODS.contains(&m.as_str()))
        .collect();
    unknown.sort();
    unknown.dedup();
    if !unknown.is_empty() {
        return Err(ApiError::bad_request(format!(
            "unknown moods {}, available: {}",
            py_list_repr(&unknown),
            py_list_repr(&crate::mix::choose::ALL_MOODS)
        )));
    }
    Ok(moods)
}

/// `_parse_track_id`
pub fn parse_track_id(body: &Value) -> Result<String, ApiError> {
    match body.get("track_id").and_then(Value::as_str) {
        Some(id) if !id.is_empty() => Ok(id.to_owned()),
        _ => Err(ApiError::bad_request("expected {\"track_id\": \"...\"}")),
    }
}

/// `_parse_track_ids`
pub fn parse_track_ids(body: &Value) -> Result<Vec<String>, ApiError> {
    body.get("track_ids")
        .and_then(strings)
        .ok_or_else(|| ApiError::bad_request("expected {\"track_ids\": [...]}"))
}

/// `_parse_move`
pub fn parse_move(body: &Value) -> Result<(String, i64), ApiError> {
    let position = body
        .get("position")
        .and_then(|p| if p.is_boolean() { None } else { p.as_i64() });
    let Some(position) = position else {
        return Err(ApiError::bad_request(
            "expected {\"track_id\": \"...\", \"position\": 0}",
        ));
    };
    Ok((parse_track_id(body)?, position))
}

/// `_parse_language`
pub fn parse_language(body: &Value) -> Result<String, ApiError> {
    match body.get("language").and_then(Value::as_str) {
        Some(language) if crate::radio::LANGUAGES.contains(&language) => Ok(language.to_owned()),
        _ => Err(ApiError::bad_request(format!(
            "expected {{\"language\": one of {}}}",
            py_list_repr(&crate::radio::LANGUAGES)
        ))),
    }
}

/// `parse_genres`
pub fn parse_genres(body: &Value) -> Result<Vec<String>, ApiError> {
    body.get("genres")
        .and_then(strings)
        .ok_or_else(|| ApiError::bad_request("expected {\"genres\": [...]}"))
}
