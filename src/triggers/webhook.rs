//! A minimal local-only HTTP listener for webhook-sourced triggers.
//!
//! No web framework: a webhook trigger accepts exactly one thing — a request
//! whose body becomes the firing's canonical input — and binds to
//! `127.0.0.1` only ("local webhook triggers"). Parsing just
//! enough HTTP/1.1 to read a request line, headers, and a `Content-Length`
//! body is a few dozen lines; a framework capable of routing, TLS, and
//! middleware would be answering questions this feature does not ask.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// Bound on a webhook body. Generous for a directive payload, small enough
/// that a misbehaving caller cannot make the listener buffer without limit.
const MAX_BODY_BYTES: usize = 256 * 1024;

/// Bind a local listener for one webhook trigger and forward each accepted
/// request's body to `occurrences`. The channel has capacity 1: a full
/// channel means an occurrence is already queued, and the caller (the
/// trigger's own overlap policy) decides what a second one means — the
/// listener's job is only to stop accepting bodies it has nowhere to put,
/// which it does by leaving the request waiting for the channel to drain
/// rather than growing memory without bound.
///
/// # Errors
/// If the port cannot be bound.
pub async fn listen(
    port: u16,
    path: String,
    occurrences: mpsc::Sender<serde_json::Value>,
    cancel: tokio_util::sync::CancellationToken,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    loop {
        tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let path = path.clone();
                let occurrences = occurrences.clone();
                tokio::spawn(async move {
                    let _ = handle_connection(stream, &path, &occurrences).await;
                });
            }
        }
    }
}

async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    expected_path: &str,
    occurrences: &mpsc::Sender<serde_json::Value>,
) -> std::io::Result<()> {
    let Some(request) = read_request(&mut stream).await? else {
        return respond(&mut stream, 400, "bad request").await;
    };

    if request.path != expected_path {
        return respond(&mut stream, 404, "no such trigger path").await;
    }

    let payload = if request.body.is_empty() {
        serde_json::Value::Null
    } else {
        match serde_json::from_slice(&request.body) {
            Ok(value) => value,
            Err(_) => {
                serde_json::Value::String(String::from_utf8_lossy(&request.body).into_owned())
            }
        }
    };

    // A full channel means a firing is already queued for this trigger. The
    // caller decides whether that means skip or replace; the listener's only
    // job is to answer honestly rather than block the socket indefinitely.
    match occurrences.try_send(payload) {
        Ok(()) => respond(&mut stream, 202, "accepted").await,
        Err(mpsc::error::TrySendError::Full(_)) => respond(&mut stream, 429, "busy").await,
        Err(mpsc::error::TrySendError::Closed(_)) => {
            respond(&mut stream, 503, "trigger stopped").await
        }
    }
}

struct ParsedRequest {
    path: String,
    body: Vec<u8>,
}

/// Read a request line, headers, and a `Content-Length` body. Returns `None`
/// for anything this minimal parser cannot make sense of.
async fn read_request(
    stream: &mut tokio::net::TcpStream,
) -> std::io::Result<Option<ParsedRequest>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());
        if let Some(position) = find_header_end(&buffer) {
            break position;
        }
        if buffer.len() > MAX_BODY_BYTES {
            return Ok(None);
        }
    };

    let header_text =
        String::from_utf8_lossy(buffer.get(..header_end).unwrap_or_default()).into_owned();
    let mut lines = header_text.split("\r\n");
    let Some(request_line) = lines.next() else {
        return Ok(None);
    };
    let mut parts = request_line.split_whitespace();
    let _method = parts.next();
    let Some(target) = parts.next() else {
        return Ok(None);
    };
    let path = target.split('?').next().unwrap_or(target).to_owned();

    let content_length: usize = lines
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        })
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0)
        .min(MAX_BODY_BYTES);

    let body_start = header_end + 4; // past the blank line's CRLFCRLF
    let mut body = buffer.get(body_start..).unwrap_or_default().to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(chunk.get(..read).unwrap_or_default());
        if body.len() > MAX_BODY_BYTES {
            return Ok(None);
        }
    }
    body.truncate(content_length);

    Ok(Some(ParsedRequest { path, body }))
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

async fn respond(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    reason: &str,
) -> std::io::Result<()> {
    let body = format!("{{\"status\":\"{reason}\"}}");
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn a_posted_json_payload_is_forwarded_as_the_occurrence() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);

        let (tx, mut rx) = mpsc::channel(1);
        let cancel = tokio_util::sync::CancellationToken::new();
        let server_cancel = cancel.clone();
        let server = tokio::spawn(listen(port, "/".to_owned(), tx, server_cancel));

        // Give the listener a moment to bind.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let body = "{\"key\":\"value\"}";
        let request = format!(
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.expect("read");
        assert!(String::from_utf8_lossy(&response).contains("202"));

        let payload = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("occurrence arrives")
            .expect("channel open");
        assert_eq!(payload["key"], "value");

        cancel.cancel();
        let _ = server.await;
    }
}
