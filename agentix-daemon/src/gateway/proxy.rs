use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use hyper::Request;
use hyper_util::rt::TokioIo;
use std::{
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::net::UnixStream;

struct ConnectionTask(tokio::task::JoinHandle<()>);

impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct BackendBody {
    incoming: hyper::body::Incoming,
    _connection: ConnectionTask,
}

impl hyper::body::Body for BackendBody {
    type Data = axum::body::Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.get_mut().incoming).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.incoming.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.incoming.size_hint()
    }
}

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

    // Keep ownership through request setup, then transfer it to the response body.
    // Dropping a JoinHandle alone detaches the driver and can leave generation running.
    let connection = ConnectionTask(tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!("backend connection closed: {e}");
        }
    }));

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
    resp.body(Body::new(BackendBody {
        incoming,
        _connection: connection,
    }))
    .unwrap_or_else(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
}
