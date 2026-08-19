//! Phase 0 compile spike : prove reqwest can consume a local mock SSE
//! stream and a separately mocked NDJSON stream.
//!
//! These are throwaway. Phase 2 replaces them with real provider contract tests
//! driven by redacted fixtures. What they establish now is that the transport
//! choices in `docs/provider-contract.md` are viable before we build on them:
//! SSE for OpenAI/Anthropic/Gemini/OpenRouter, NDJSON for Ollama.
//!
//! No network. No credentials. `wiremock` binds a local port (AGENTS.md §7).

// AGENTS.md §7: tests may panic freely — clarity beats ceremony, and a panicking
// assertion is a failing test, not a corrupted terminal. Clippy's
// `allow-*-in-tests` options in `clippy.toml` cover unwrap/expect/panic but not
// indexing, and integration tests are separate crates without `cfg(test)`, so
// the allowance has to be stated per file.
#![allow(clippy::indexing_slicing)]

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// An SSE body shaped like the real thing, including the two details that break
/// naive parsers (`docs/provider-contract.md` §0):
///
/// - a comment frame keep-alive, which `OpenRouter` really sends
/// - a `[DONE]` sentinel that is not JSON at all
const SSE_BODY: &str = concat!(
    ": OPENROUTER PROCESSING\n\n",
    "event: message_start\n",
    "data: {\"type\":\"message_start\"}\n\n",
    ": OPENROUTER PROCESSING\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
    "event: message_stop\n",
    "data: {\"type\":\"message_stop\"}\n\n",
    "data: [DONE]\n\n",
);

/// An Ollama-shaped NDJSON body: one JSON object per line, terminal chunk
/// carrying `done: true` (`docs/provider-contract.md` §5).
const NDJSON_BODY: &str = concat!(
    "{\"model\":\"m\",\"done\":false,\"message\":{\"role\":\"assistant\",\"content\":\"Hel\"}}\n",
    "{\"model\":\"m\",\"done\":false,\"message\":{\"role\":\"assistant\",\"content\":\"lo\"}}\n",
    "{\"model\":\"m\",\"done\":true,\"done_reason\":\"stop\",\"eval_count\":2,\"prompt_eval_count\":7}\n",
);

#[tokio::test]
async fn reqwest_consumes_sse_and_skips_comment_frames() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(SSE_BODY, "text/event-stream")
                .insert_header("cache-control", "no-cache"),
        )
        .mount(&server)
        .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", server.uri()))
        .send()
        .await
        .expect("mock server should respond");

    let mut events = response.bytes_stream().eventsource();
    let mut names = Vec::new();
    let mut text = String::new();

    while let Some(event) = events.next().await {
        let event = event.expect("SSE frame should decode");

        // The transport layer surfaces `[DONE]` as data; recognising it is the
        // provider layer's job, not the decoder's ( keeps these apart).
        if event.data == "[DONE]" {
            names.push("[DONE]".to_owned());
            break;
        }

        names.push(event.event.clone());

        if event.event == "content_block_delta" {
            let value: serde_json::Value =
                serde_json::from_str(&event.data).expect("delta data should be JSON");
            if let Some(fragment) = value["delta"]["text"].as_str() {
                text.push_str(fragment);
            }
        }
    }

    // Comment frames must be invisible to the caller, not merely tolerated.
    assert_eq!(
        names,
        vec![
            "message_start",
            "content_block_delta",
            "content_block_delta",
            "message_stop",
            "[DONE]",
        ],
        "comment frames must be skipped without dropping real events"
    );

    // Proves deltas accumulate rather than replace.
    assert_eq!(text, "Hello");
}

/// Proves reqwest can consume an NDJSON body and that an incremental line
/// decoder reassembles it. It does **not** prove chunk-boundary safety —
/// wiremock likely delivers this body in one chunk. That claim belongs to
/// `ndjson_line_split_survives_arbitrary_chunk_boundaries` below, which forces
/// a split at every byte position.
#[tokio::test]
async fn reqwest_consumes_ndjson_body() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(NDJSON_BODY, "application/x-ndjson"))
        .mount(&server)
        .await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/chat", server.uri()))
        .send()
        .await
        .expect("mock server should respond");

    // Deliberately hand-rolled: NDJSON has no decoder crate in the dependency
    // shortlist, and this proves an incremental line split is all it needs.
    // A real implementation must never assume a chunk is a whole line.
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::<u8>::new();
    let mut lines = Vec::<String>::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk should arrive");
        buffer.extend_from_slice(&chunk);

        while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = buffer.drain(..=newline).collect();
            let line = String::from_utf8(line).expect("body is UTF-8");
            let line = line.trim_end();
            if !line.is_empty() {
                lines.push(line.to_owned());
            }
        }
    }

    // A trailing unterminated line must not be silently dropped, and must not
    // be parsed as if it were complete.
    assert!(
        buffer.is_empty(),
        "well-formed NDJSON leaves no unterminated tail"
    );
    assert_eq!(lines.len(), 3);

    let mut text = String::new();
    let mut done_reason = None;

    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line).expect("each line is JSON");
        if let Some(fragment) = value["message"]["content"].as_str() {
            text.push_str(fragment);
        }
        if value["done"].as_bool() == Some(true) {
            done_reason = value["done_reason"].as_str().map(str::to_owned);
        }
    }

    assert_eq!(text, "Hello");
    assert_eq!(done_reason.as_deref(), Some("stop"));
}

/// The incremental decoder must not care where chunk boundaries land. Splitting
/// mid-line is the real-world case that breaks naive `split('\n')` code.
#[test]
fn ndjson_line_split_survives_arbitrary_chunk_boundaries() {
    for split_at in 1..NDJSON_BODY.len() {
        let (head, tail) = NDJSON_BODY.split_at(split_at);

        let mut buffer = Vec::<u8>::new();
        let mut lines = Vec::<String>::new();

        for chunk in [head.as_bytes(), tail.as_bytes()] {
            buffer.extend_from_slice(chunk);
            while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = buffer.drain(..=newline).collect();
                let line = String::from_utf8(line).expect("body is UTF-8");
                if !line.trim_end().is_empty() {
                    lines.push(line.trim_end().to_owned());
                }
            }
        }

        assert_eq!(
            lines.len(),
            3,
            "splitting the body at byte {split_at} must still yield 3 lines"
        );
    }
}
