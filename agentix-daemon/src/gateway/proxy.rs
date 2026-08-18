use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use hyper::Request;
use hyper_util::rt::TokioIo;
use std::path::Path;
use tokio::net::UnixStream;

/// Forward an HTTP request to a backend Unix socket.
///
/// Connects fresh per-request (no pooling needed for local IPC).
/// Streams the response body without buffering so SSE works correctly.
pub async fn forward(
    socket: &Path,
    method: Method,
    uri: &str,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(socket = %socket.display(), err = %e, "backend socket unavailable");
            let mut resp = (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("backend unavailable: {e}"),
            )
                .into_response();
            resp.headers_mut()
                .insert("retry-after", HeaderValue::from_static("1"));
            return resp;
        }
    };

    let io = TokioIo::new(stream);
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("backend handshake failed: {e}"),
            )
                .into_response()
        }
    };

    // Drive the connection in the background
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!("backend connection closed: {e}");
        }
    });

    // Build a hyper request from the parts we have
    let uri_parsed: Uri = match uri.parse() {
        Ok(u) => u,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("bad uri: {e}")).into_response()
        }
    };

    let mut builder = Request::builder().method(method).uri(uri_parsed);
    // Copy headers; hyper requires a `host` header for HTTP/1.1
    for (name, value) in &headers {
        builder = builder.header(name, value);
    }
    if !headers.contains_key("host") {
        builder = builder.header("host", "localhost");
    }

    let hyper_req = match builder.body(body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("request build failed: {e}"),
            )
                .into_response()
        }
    };

    let hyper_resp = match sender.send_request(hyper_req).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("backend request failed: {e}"),
            )
                .into_response()
        }
    };

    // Convert hyper response to axum response, preserving status + headers + streaming body
    let (parts, incoming) = hyper_resp.into_parts();
    let mut resp = Response::builder().status(parts.status);
    for (name, value) in &parts.headers {
        resp = resp.header(name, value);
    }
    resp.body(Body::new(incoming))
        .unwrap_or_else(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
}
