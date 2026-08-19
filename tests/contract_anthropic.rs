//! Anthropic Messages adapter contract tests (plan Phase 6).

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;

use smed::core::error::{ProviderError, ReasonCode};
use smed::core::event::{FinishReason, ProviderEvent};
use smed::core::message::{CanonicalMessage, ContentBlock, ToolCall, ToolResult};
use smed::core::model::{ModelId, ProviderId};
use smed::core::provider::{Provider, ProviderRequest};
use smed::core::secrets::{
    Credential, CredentialKind, OAuthCredential, ResolvedCredential, Secret, SecretError,
    SecretSource, SecretStore,
};
use smed::core::tool::ToolDefinition;
use smed::providers::anthropic::{AnthropicProvider, DEFAULT_MODEL};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Debug)]
struct FakeSecrets(Option<&'static str>);

impl SecretStore for FakeSecrets {
    fn resolve(
        &self,
        provider: &ProviderId,
        _kind: CredentialKind,
    ) -> Result<ResolvedCredential, SecretError> {
        assert_eq!(provider.as_str(), "anthropic");
        self.0.map_or_else(
            || {
                Err(SecretError::NotFound {
                    provider: provider.clone(),
                })
            },
            |value| {
                Ok(ResolvedCredential {
                    credential: Credential::ApiKey(Secret::new(value.to_owned())),
                    source: SecretSource::Environment,
                })
            },
        )
    }

    fn store(&self, _provider: &ProviderId, _credential: Credential) -> Result<(), SecretError> {
        Ok(())
    }

    fn delete(&self, _provider: &ProviderId) -> Result<(), SecretError> {
        Ok(())
    }
}

fn request() -> ProviderRequest {
    ProviderRequest {
        model: ModelId::new(DEFAULT_MODEL),
        messages: vec![CanonicalMessage::user("hello")],
        system: Some("guard every effect".to_owned()),
        tools: Vec::new(),
        images: smed::core::image::ImageSidecar::new(),
    }
}

async fn run(
    server: &MockServer,
    secret: Option<&'static str>,
    request: ProviderRequest,
) -> (
    Vec<ProviderEvent>,
    Result<smed::core::provider::ProviderCompletion, ProviderError>,
) {
    let provider =
        AnthropicProvider::new(Arc::new(FakeSecrets(secret))).with_base_url(server.uri());
    let (tx, mut rx) = mpsc::channel(64);
    let task =
        tokio::spawn(async move { provider.stream(request, tx, CancellationToken::new()).await });
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    (events, task.await.expect("adapter task"))
}

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_owned(), "text/event-stream")
}

async fn mount(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(response)
        .mount(server)
        .await;
}

#[tokio::test]
async fn headers_and_canonical_tool_history_use_the_messages_contract() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .and(header("x-api-key", "sk-ant-test"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(sse(concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )))
        .mount(&server)
        .await;
    let request = ProviderRequest {
        model: ModelId::new(DEFAULT_MODEL),
        messages: vec![
            CanonicalMessage::user("read it"),
            CanonicalMessage::assistant(
                vec![ContentBlock::ToolCall(ToolCall {
                    id: "call_from_openai".to_owned(),
                    name: "read_file".to_owned(),
                    arguments: serde_json::json!({"path":"src/lib.rs"}),
                    provider_signature: None,
                })],
                ProviderId::new("openai"),
                ModelId::new("gpt-4o-mini"),
            ),
            CanonicalMessage::tool_result(
                "call_from_openai",
                "read_file",
                ToolResult::ok("contents"),
            ),
        ],
        system: Some("system rules".to_owned()),
        tools: vec![ToolDefinition {
            name: "read_file".to_owned(),
            description: "read".to_owned(),
            schema: serde_json::json!({
                "$schema":"https://json-schema.org/draft/2020-12/schema",
                "type":"object",
                "properties":{"path":{"type":"string"}},
                "required":["path"],
                "additionalProperties":false
            }),
        }],
        images: smed::core::image::ImageSidecar::new(),
    };

    let (_, outcome) = run(&server, Some("sk-ant-test"), request).await;
    outcome.expect("stream succeeds");
    let requests = server.received_requests().await.expect("request recorded");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
    assert_eq!(body["system"][0]["text"], "system rules");
    assert_eq!(body["messages"][1]["role"], "assistant");
    assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(body["messages"][1]["content"][0]["id"], "call_from_openai");
    assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(
        body["messages"][2]["content"][0]["tool_use_id"],
        "call_from_openai"
    );
    assert!(body["tools"][0]["input_schema"].get("$schema").is_none());
    assert!(!String::from_utf8_lossy(&requests[0].body).contains("sk-ant-test"));
}

#[tokio::test]
async fn indexed_partial_json_thinking_and_cumulative_usage_normalize_once() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse(concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"private chain\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_b\",\"name\":\"read_file\",\"input\":{}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_a\",\"name\":\"list_files\",\"input\":{}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\".\\\"}\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"src/lib.rs\\\"}\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":3,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_c\",\"name\":\"list_files\",\"input\":{}}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":3}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":9}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )),
    )
    .await;

    let (events, outcome) = run(&server, Some("key"), request()).await;
    let completion = outcome.expect("stream succeeds");
    assert_eq!(completion.reason, FinishReason::ToolCalls);
    assert_eq!(completion.usage.expect("usage").input_tokens, 12);
    assert_eq!(completion.usage.expect("usage").output_tokens, 9);
    assert!(!events.iter().any(
        |event| matches!(event, ProviderEvent::TextDelta { text, .. } if text.contains("private"))
    ));
    assert!(events.iter().any(
        |event| matches!(event, ProviderEvent::ReasoningDelta { text } if text == "private chain")
    ));
    let calls = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ToolCallCompleted { call } => Some(call),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].id, "toolu_a");
    assert_eq!(calls[0].arguments, serde_json::json!({"path":"."}));
    assert_eq!(calls[1].id, "toolu_b");
    assert_eq!(calls[1].arguments, serde_json::json!({"path":"src/lib.rs"}));
    assert_eq!(calls[2].id, "toolu_c");
    assert_eq!(calls[2].arguments, serde_json::json!({}));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::Usage { .. }))
            .count(),
        1,
        "cumulative usage must be emitted once"
    );
}

#[tokio::test]
async fn unknown_and_fallback_events_are_retained_without_breaking_the_stream() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse(concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: future_event\ndata: {\"type\":\"future_event\",\"new\":true}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"fallback\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )),
    )
    .await;
    let (events, outcome) = run(&server, Some("key"), request()).await;
    assert!(outcome.is_ok());
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::UnknownUpstream { kind } if kind == "future_event")
    }));
}

#[tokio::test]
async fn a_malformed_known_event_is_a_protocol_failure_at_the_corrupt_frame() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse(concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )),
    )
    .await;

    let (events, outcome) = run(&server, Some("key"), request()).await;
    assert_eq!(
        outcome.expect_err("malformed known event").reason_code(),
        ReasonCode::ProviderProtocol
    );
    assert!(!events.iter().any(|event| {
        matches!(event, ProviderEvent::UnknownUpstream { kind } if kind == "message_delta")
    }));
}

#[tokio::test]
async fn errors_and_missing_credentials_fail_before_or_without_leaking_bodies() {
    let server = MockServer::start().await;
    mount(&server, sse("")).await;
    let (_, missing) = run(&server, None, request()).await;
    assert_eq!(
        missing.expect_err("missing key").reason_code(),
        ReasonCode::ProviderAuth
    );
    assert!(
        server
            .received_requests()
            .await
            .expect("requests")
            .is_empty()
    );

    let server = MockServer::start().await;
    mount(
        &server,
        ResponseTemplate::new(401).set_body_raw(
            r#"{"type":"error","error":{"type":"authentication_error","message":"bad sk-ant-leak"},"request_id":"req_1"}"#,
            "application/json",
        ),
    )
    .await;
    let (_, failure) = run(&server, Some("key"), request()).await;
    let failure = failure.expect_err("401");
    assert_eq!(failure.reason_code(), ReasonCode::ProviderAuth);
    assert!(!format!("{failure} {failure:?}").contains("sk-ant-leak"));
}

#[tokio::test]
async fn mid_stream_overload_is_classified_without_retrying() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse("event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"busy\"}}\n\n"),
    )
    .await;
    let (_, outcome) = run(&server, Some("key"), request()).await;
    // Was ProviderRateLimit, which reported an upstream capacity problem as
    // the caller's exhausted quota — a user with an untouched limit was told
    // they had hit it. The no-retry guarantee below is what this test is for
    // and is unchanged.
    assert_eq!(
        outcome.expect_err("overload").reason_code(),
        ReasonCode::ProviderOverloaded
    );
    assert_eq!(
        server.received_requests().await.expect("requests").len(),
        1,
        "the adapter must never retry a stream"
    );
}

#[tokio::test]
async fn an_image_encodes_to_the_documented_base64_block() {
    use smed::core::image::ImageBytes;
    use smed::core::message::ContentBlock;

    // The shape confirmed against current documentation on 2026-07-25
    // (`docs/provider-contract.md` §5.5). Asserted on the body the adapter
    // actually sent, not on a struct: a field rename would pass a struct test
    // and still break the request.
    let server = MockServer::start().await;
    mount(
        &server,
        ResponseTemplate::new(200).set_body_raw(
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            "text/event-stream",
        ),
    )
    .await;

    let mut message = CanonicalMessage::user("what is this?");
    message.blocks.push(ContentBlock::ImageRef {
        media_type: "image/png".to_owned(),
        source: "shot.png".to_owned(),
    });
    let mut images = smed::core::image::ImageSidecar::new();
    images.insert(
        "shot.png".to_owned(),
        ImageBytes {
            media_type: "image/png".to_owned(),
            bytes: std::sync::Arc::from([1_u8, 2, 3].as_slice()),
        },
    );
    let request = ProviderRequest {
        model: ModelId::new("claude-opus-5"),
        messages: vec![message],
        system: None,
        tools: Vec::new(),
        images,
    };

    let (_, _outcome) = run(&server, Some("sk-ant-test"), request).await;
    let requests = server.received_requests().await.expect("request recorded");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
    let block = body
        .pointer("/messages/0/content/1")
        .expect("the image block travels beside the text");
    assert_eq!(block["type"], "image");
    assert_eq!(block["source"]["type"], "base64");
    assert_eq!(block["source"]["media_type"], "image/png");
    assert_eq!(block["source"]["data"], "AQID");
}

#[tokio::test]
async fn an_image_whose_bytes_are_absent_is_never_sent_as_an_empty_block() {
    use smed::core::message::ContentBlock;

    // An empty sidecar means the gate already projected this into a placeholder.
    // Encoding it anyway would tell the model a picture is attached when nothing
    // was sent — the exact class of lie AGENTS.md §1.3 forbids.
    let server = MockServer::start().await;
    mount(
        &server,
        ResponseTemplate::new(200).set_body_raw(
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            "text/event-stream",
        ),
    )
    .await;

    let mut message = CanonicalMessage::user("what is this?");
    message.blocks.push(ContentBlock::ImageRef {
        media_type: "image/png".to_owned(),
        source: "missing.png".to_owned(),
    });
    let request = ProviderRequest {
        model: ModelId::new("claude-opus-5"),
        messages: vec![message],
        system: None,
        tools: Vec::new(),
        images: smed::core::image::ImageSidecar::new(),
    };

    let (_, _outcome) = run(&server, Some("sk-ant-test"), request).await;
    let requests = server.received_requests().await.expect("request recorded");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
    assert!(
        body.pointer("/messages/0/content/1").is_none(),
        "no second block may appear: {body}"
    );
}

#[test]
fn every_offered_model_declares_only_implemented_capabilities() {
    let provider = AnthropicProvider::new(Arc::new(FakeSecrets(None)));
    let models = provider.models();
    assert_eq!(models.len(), 3);
    for model in models {
        assert!(model.capabilities.streaming);
        assert!(model.capabilities.tools);
        assert!(
            model.capabilities.images_in,
            "image translation is built and confirmed ; a model \
             that stops declaring it would silently start sending placeholders"
        );
        assert!(!model.capabilities.reasoning_controls);
    }
}

#[tokio::test]
async fn subscription_oauth_sends_claude_code_headers_and_identity_block() {
    #[derive(Debug)]
    struct OAuthSecrets;
    impl SecretStore for OAuthSecrets {
        fn resolve(
            &self,
            provider: &ProviderId,
            kind: CredentialKind,
        ) -> Result<ResolvedCredential, SecretError> {
            if kind == CredentialKind::OAuth {
                Ok(ResolvedCredential {
                    credential: Credential::OAuth(OAuthCredential::new(
                        Secret::new("sk-ant-oat-test".to_owned()),
                        Secret::new("refresh".to_owned()),
                        9_999_999_999,
                        "acct".to_owned(),
                    )),
                    source: SecretSource::Environment,
                })
            } else {
                Err(SecretError::NotFound {
                    provider: provider.clone(),
                })
            }
        }
        fn store(&self, _p: &ProviderId, _c: Credential) -> Result<(), SecretError> {
            Ok(())
        }
        fn delete(&self, _p: &ProviderId) -> Result<(), SecretError> {
            Ok(())
        }
    }

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(sse(concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(Arc::new(OAuthSecrets)).with_base_url(server.uri());
    let (tx, mut rx) = mpsc::channel(64);
    let task = tokio::spawn(async move {
        provider
            .stream(request(), tx, CancellationToken::new())
            .await
    });
    while rx.recv().await.is_some() {}
    task.await.expect("task").expect("stream succeeds");

    let requests = server.received_requests().await.expect("request recorded");
    let req = &requests[0];
    let auth = req.headers.get("authorization").expect("auth header");
    assert_eq!(auth.to_str().unwrap(), "Bearer sk-ant-oat-test");
    let beta = req.headers.get("anthropic-beta").expect("beta header");
    assert_eq!(
        beta.to_str().unwrap(),
        "claude-code-20250219,oauth-2025-04-20"
    );
    let ua = req.headers.get("user-agent").expect("user agent header");
    assert_eq!(ua.to_str().unwrap(), "claude-cli/2.0.0 (external, cli)");

    let body: serde_json::Value = serde_json::from_slice(&req.body).expect("JSON body");
    assert_eq!(
        body["system"][0]["text"],
        "You are Claude Code, Anthropic's official CLI for Claude."
    );
    assert_eq!(body["system"][1]["text"], "guard every effect");
}
