//! OpenAI adapter contract tests.
//!
//! Local mock only. No network, no credentials — `wiremock` binds a loopback
//! port (AGENTS.md §7). The live smoke test lives in `tests/live_openai.rs` and
//! is `#[ignore]`d.
//!
//! Fixtures are synthetic, built from `docs/provider-contract.md` §1, which was
//! written from the official OpenAPI specification. They are handwritten rather
//! than captured because Phase 2 has no live key: `tests/fixtures/providers/README.md`
//! permits synthetic fixtures for exactly this, provided they exercise a
//! specific decoder state.

#![allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;
use std::time::Duration;

use smed::core::error::{ProviderError, ReasonCode};
use smed::core::event::{FinishReason, ProviderEvent};
use smed::core::message::{CanonicalMessage, ContentBlock, ToolCall, ToolResult};
use smed::core::model::{ModelId, ProviderId};
use smed::core::provider::{Provider, ProviderRequest};
use smed::core::secrets::{
    Credential, CredentialKind, ResolvedCredential, Secret, SecretError, SecretSource, SecretStore,
};
use smed::providers::openai::OpenAiProvider;
use smed::tools::ToolRegistry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A store that yields a fixed fake key. The real keychain is never touched by
/// the test suite: a suite that prompts for a login password is one nobody runs.
#[derive(Debug)]
struct FakeSecrets {
    secret: Option<&'static str>,
}

impl FakeSecrets {
    fn with(secret: &'static str) -> Arc<dyn SecretStore> {
        Arc::new(Self {
            secret: Some(secret),
        })
    }

    fn missing() -> Arc<dyn SecretStore> {
        Arc::new(Self { secret: None })
    }
}

impl SecretStore for FakeSecrets {
    fn resolve(
        &self,
        provider: &ProviderId,
        _kind: CredentialKind,
    ) -> Result<ResolvedCredential, SecretError> {
        match self.secret {
            Some(value) => Ok(ResolvedCredential {
                credential: Credential::ApiKey(Secret::new(value.to_owned())),
                source: SecretSource::Environment,
            }),
            None => Err(SecretError::NotFound {
                provider: provider.clone(),
            }),
        }
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
        model: ModelId::new("gpt-4o-mini"),
        messages: vec![CanonicalMessage::user("hello")],
        system: None,
        tools: Vec::new(),
        images: smed::core::image::ImageSidecar::new(),
    }
}

/// Drive the adapter and collect every event it emits.
async fn run(
    server: &MockServer,
    secrets: Arc<dyn SecretStore>,
) -> (
    Vec<ProviderEvent>,
    Result<smed::core::provider::ProviderCompletion, smed::core::error::ProviderError>,
) {
    run_request(server, secrets, request()).await
}

async fn run_request(
    server: &MockServer,
    secrets: Arc<dyn SecretStore>,
    request: ProviderRequest,
) -> (
    Vec<ProviderEvent>,
    Result<smed::core::provider::ProviderCompletion, smed::core::error::ProviderError>,
) {
    let provider = OpenAiProvider::new(secrets).with_base_url(server.uri());
    let (tx, mut rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();

    let task = tokio::spawn(async move { provider.stream(request, tx, cancel).await });

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }

    (events, task.await.expect("adapter task"))
}

#[tokio::test]
async fn tools_and_function_results_use_the_responses_api_contract() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse("event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"incomplete_details\":null,\"usage\":null}}\n\n"),
    )
    .await;
    let request = ProviderRequest {
        model: ModelId::new("gpt-4o-mini"),
        messages: vec![
            CanonicalMessage::user("read the file"),
            CanonicalMessage::assistant(
                vec![ContentBlock::ToolCall(ToolCall {
                    id: "call_1".to_owned(),
                    name: "read_file".to_owned(),
                    arguments: serde_json::json!({ "path": "src/lib.rs" }),
                    provider_signature: None,
                })],
                ProviderId::new("openai"),
                ModelId::new("gpt-4o-mini"),
            ),
            CanonicalMessage::tool_result("call_1", "read_file", ToolResult::ok("contents")),
        ],
        system: Some("use tools".to_owned()),
        tools: vec![smed::core::tool::ToolDefinition {
            name: "read_file".to_owned(),
            description: "read a file".to_owned(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
                "additionalProperties": false
            }),
        }],
        images: smed::core::image::ImageSidecar::new(),
    };

    let (_events, outcome) = run_request(&server, FakeSecrets::with("sk-test"), request).await;
    outcome.expect("stream succeeds");
    let requests = server.received_requests().await.expect("request recorded");
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("JSON request body");
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["name"], "read_file");
    assert_eq!(body["tools"][0]["strict"], true);
    assert_eq!(body["input"][1]["type"], "function_call");
    assert_eq!(body["input"][1]["call_id"], "call_1");
    assert_eq!(body["input"][2]["type"], "function_call_output");
    assert_eq!(body["input"][2]["call_id"], "call_1");
}

#[tokio::test]
async fn every_builtin_tool_emits_strict_compatible_openai_parameters() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse("event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"incomplete_details\":null,\"usage\":null}}\n\n"),
    )
    .await;

    let request = ProviderRequest {
        model: ModelId::new("gpt-4o-mini"),
        messages: vec![CanonicalMessage::user("inspect the repository")],
        system: None,
        tools: ToolRegistry::builtins().definitions(),
        images: smed::core::image::ImageSidecar::new(),
    };

    let (_events, outcome) = run_request(&server, FakeSecrets::with("sk-test"), request).await;
    outcome.expect("stream succeeds");

    let requests = server.received_requests().await.expect("request recorded");
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("JSON request body");
    let tools = body["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), ToolRegistry::builtins().definitions().len());

    for tool in tools {
        let name = tool["name"].as_str().expect("tool name");
        let parameters = tool["parameters"].as_object().expect("parameters object");
        assert!(
            !parameters.contains_key("$schema"),
            "{name} leaked a JSON Schema dialect marker onto the OpenAI wire"
        );
        assert_eq!(parameters.get("additionalProperties"), Some(&false.into()));

        let properties = parameters["properties"]
            .as_object()
            .expect("properties object");
        let required = parameters["required"]
            .as_array()
            .expect("required array")
            .iter()
            .map(|field| field.as_str().expect("required field"))
            .collect::<std::collections::BTreeSet<_>>();
        let declared = properties
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            required, declared,
            "strict OpenAI tool {name} must require every declared property"
        );
    }
}

#[tokio::test]
async fn a_bare_error_frame_reports_its_code_not_the_generic_event_type() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse("event: error\ndata: {\"type\":\"error\",\"code\":\"invalid_function_parameters\",\"message\":\"schema rejected\",\"param\":\"tools[0].parameters\",\"sequence_number\":1}\n\n"),
    )
    .await;

    let (_events, outcome) = run(&server, FakeSecrets::with("sk-test")).await;
    let error = outcome.expect_err("error frame must fail the request");
    assert_eq!(error.reason_code(), ReasonCode::ProviderProtocol);
    assert!(
        error.to_string().contains("invalid_function_parameters"),
        "provider code was lost: {error}"
    );
}

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_owned(), "text/event-stream")
}

async fn mount(server: &MockServer, template: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(template)
        .mount(server)
        .await;
}

#[tokio::test]
async fn text_streams_and_usage_is_reported() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse(concat!(
            "event: response.created\ndata: {\"type\":\"response.created\"}\n\n",
            "event: response.reasoning_summary_text.delta\ndata: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"checked files\"}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\",\"sequence_number\":1}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\",\"sequence_number\":2}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"incomplete_details\":null,\"usage\":{\"input_tokens\":11,\"input_tokens_details\":{\"cached_tokens\":8},\"output_tokens\":3,\"output_tokens_details\":{\"reasoning_tokens\":1},\"total_tokens\":14}}}\n\n",
            "event: done\ndata: [DONE]\n\n",
        ))
        .insert_header("x-ratelimit-limit-tokens", "100")
        .insert_header("x-ratelimit-remaining-tokens", "25")
        .insert_header("x-ratelimit-reset-tokens", "1700000000"),
    )
    .await;

    let (events, outcome) = run(&server, FakeSecrets::with("sk-test")).await;
    let completion = outcome.expect("stream succeeds");

    assert_eq!(completion.reason, FinishReason::Stop);

    let text: String = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello");
    assert!(events.iter().any(
        |event| matches!(event, ProviderEvent::ReasoningDelta { text } if text == "checked files")
    ));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            ProviderEvent::Quota { snapshot }
                if snapshot.windows.iter().any(|window| {
                    window.label == "tokens"
                        && (window.used_fraction - 0.75).abs() < f32::EPSILON
                })
        )
    }));

    // The `_details` are breakdowns: cached ⊆ input, reasoning ⊆ output. Adding
    // them would report 19/4 instead of 11/3.
    let usage = completion.usage.expect("usage reported");
    assert_eq!(usage.input_tokens, 11, "cached tokens must not be added on");
    assert_eq!(
        usage.output_tokens, 3,
        "reasoning tokens must not be added on"
    );
}

#[tokio::test]
async fn the_authorization_header_carries_the_key_and_nothing_else_does() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer sk-test-key"))
        .respond_with(sse(
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"incomplete_details\":null,\"usage\":null}}\n\n",
        ))
        .mount(&server)
        .await;

    // The matcher above is the assertion: without the exact bearer header,
    // wiremock returns 404 and the stream fails.
    let (_events, outcome) = run(&server, FakeSecrets::with("sk-test-key")).await;
    assert!(outcome.is_ok(), "the bearer header must be sent verbatim");

    // And the key must not appear in the request body.
    let requests = server.received_requests().await.expect("requests recorded");
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(
        !body.contains("sk-test-key"),
        "the credential leaked into the request body: {body}"
    );
}

#[tokio::test]
async fn a_missing_credential_fails_as_auth_before_any_request_is_sent() {
    let server = MockServer::start().await;
    mount(&server, sse("")).await;

    let (_events, outcome) = run(&server, FakeSecrets::missing()).await;
    assert_eq!(
        outcome.expect_err("must fail").reason_code(),
        ReasonCode::ProviderAuth
    );

    let requests = server.received_requests().await.expect("requests recorded");
    assert!(
        requests.is_empty(),
        "no request may be sent without a credential"
    );
}

#[tokio::test]
async fn http_401_maps_to_auth_and_does_not_echo_the_body() {
    let server = MockServer::start().await;
    mount(
        &server,
        ResponseTemplate::new(401).set_body_raw(
            r#"{"error":{"type":"invalid_request_error","message":"Incorrect API key sk-leaked-abc provided","param":null,"code":"invalid_api_key"}}"#,
            "application/json",
        ),
    )
    .await;

    let (_events, outcome) = run(&server, FakeSecrets::with("sk-test")).await;
    let error = outcome.expect_err("must fail");

    assert_eq!(error.reason_code(), ReasonCode::ProviderAuth);
    let rendered = format!("{error} {error:?}");
    assert!(
        !rendered.contains("sk-leaked-abc"),
        "a credential echoed by the provider leaked into our error: {rendered}"
    );
}

#[tokio::test]
async fn http_429_maps_to_rate_limit() {
    let server = MockServer::start().await;
    mount(
        &server,
        ResponseTemplate::new(429)
            .set_body_raw(r#"{"error":{"type":"rate_limit_error","message":"slow down","param":null,"code":"rate_limit_exceeded"}}"#, "application/json")
            .insert_header("retry-after", "30"),
    )
    .await;

    let (_events, outcome) = run(&server, FakeSecrets::with("sk-test")).await;
    let error = outcome.expect_err("must fail");
    assert_eq!(error.reason_code(), ReasonCode::ProviderRateLimit);
    assert!(matches!(
        error,
        ProviderError::RateLimit {
            retry_after_seconds: Some(30)
        }
    ));
}

/// The trap: HTTP 200, then a rate limit inside the stream.
///
/// Confirmed from the spec — `ResponseErrorCode` includes `rate_limit_exceeded`.
/// Mapping this to a generic protocol error would send the user retrying into
/// the same wall.
#[tokio::test]
async fn a_rate_limit_arriving_mid_stream_under_http_200_still_maps_to_rate_limit() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse(concat!(
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"slow down\"},\"incomplete_details\":null,\"usage\":null}}\n\n",
        )),
    )
    .await;

    let (events, outcome) = run(&server, FakeSecrets::with("sk-test")).await;

    assert_eq!(
        outcome.expect_err("must fail").reason_code(),
        ReasonCode::ProviderRateLimit,
        "HTTP 200 does not mean success: the body is the authority"
    );

    // Output produced before the failure is still real and was emitted.
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::TextDelta { .. })),
        "text produced before the failure must still be emitted"
    );
}

#[tokio::test]
async fn an_incomplete_response_is_neither_success_nor_failure() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse(concat!(
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"truncated\"}\n\n",
            "event: response.incomplete\ndata: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"error\":null,\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"usage\":{\"input_tokens\":5,\"input_tokens_details\":{\"cached_tokens\":0},\"output_tokens\":9,\"output_tokens_details\":{\"reasoning_tokens\":0},\"total_tokens\":14}}}\n\n",
        )),
    )
    .await;

    let (_events, outcome) = run(&server, FakeSecrets::with("sk-test")).await;
    let completion = outcome.expect("incomplete is not an error");

    assert_eq!(
        completion.reason,
        FinishReason::Incomplete,
        "a truncated answer must not report Stop"
    );
    assert_eq!(completion.usage.expect("usage").output_tokens, 9);
}

#[tokio::test]
async fn tool_arguments_arrive_as_fragments_and_parse_at_the_completion_boundary() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse(concat!(
            "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read_file\",\"arguments\":\"\"}}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"pa\"}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"th\\\": \\\"a.rs\\\"}\"}\n\n",
            "event: response.function_call_arguments.done\ndata: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\": \\\"a.rs\\\"}\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"incomplete_details\":null,\"usage\":null}}\n\n",
        )),
    )
    .await;

    let (events, outcome) = run(&server, FakeSecrets::with("sk-test")).await;
    outcome.expect("stream succeeds");

    // Keyed by call_id, not the fc_ item id: the result must quote call_id.
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::ToolCallStarted { id, name } if id == "call_1" && name == "read_file"
    )));

    let fragments: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ToolArgumentsDelta { fragment, .. } => Some(fragment.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(fragments.len(), 2, "arguments must arrive as fragments");
    assert!(
        events.iter().all(|event| !matches!(
            event,
            ProviderEvent::ToolArgumentsDelta { id, .. } if id != "call_1"
        )),
        "every argument fragment must use the canonical call_id"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(fragments[0]).is_err(),
        "a fragment must not be independently parseable — that is the whole point"
    );

    let completed = events
        .iter()
        .find_map(|event| match event {
            ProviderEvent::ToolCallCompleted { call } => Some(call),
            _ => None,
        })
        .expect("a completed tool call");
    assert_eq!(completed.id, "call_1");
    assert_eq!(completed.name, "read_file");
    assert_eq!(completed.arguments["path"], "a.rs");
}

#[tokio::test]
async fn a_function_call_closed_only_by_output_item_done_still_completes() {
    // The subscription (Codex) backend can deliver a tool call as
    // `output_item.added` + `output_item.done` carrying the full arguments,
    // with no `function_call_arguments.done` of its own. The decoder must close
    // the call from the terminal item rather than end the run reporting a tool
    // call the accumulator never received.
    let server = MockServer::start().await;
    mount(
        &server,
        sse(concat!(
            "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"list_dir\",\"arguments\":\"\"}}\n\n",
            "event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"list_dir\",\"arguments\":\"{\\\"path\\\": \\\".\\\"}\"}}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"incomplete_details\":null,\"usage\":null}}\n\n",
        )),
    )
    .await;

    let (events, outcome) = run(&server, FakeSecrets::with("sk-test")).await;
    outcome.expect("stream succeeds");

    let completed = events
        .iter()
        .find_map(|event| match event {
            ProviderEvent::ToolCallCompleted { call } => Some(call),
            _ => None,
        })
        .expect("the tool call must complete even without an arguments.done event");
    assert_eq!(completed.id, "call_1", "the result must quote call_id");
    assert_eq!(completed.name, "list_dir");
    assert_eq!(completed.arguments["path"], ".");
}

#[tokio::test]
async fn a_call_completed_by_arguments_done_is_not_double_closed_by_output_item_done() {
    // The API-key path delivers both `function_call_arguments.done` and a
    // trailing `output_item.done` for the same item. The fallback close must be
    // idempotent: exactly one completed call, not two.
    let server = MockServer::start().await;
    mount(
        &server,
        sse(concat!(
            "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"list_dir\",\"arguments\":\"\"}}\n\n",
            "event: response.function_call_arguments.done\ndata: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"name\":\"list_dir\",\"arguments\":\"{\\\"path\\\": \\\".\\\"}\"}\n\n",
            "event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"list_dir\",\"arguments\":\"{\\\"path\\\": \\\".\\\"}\"}}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"incomplete_details\":null,\"usage\":null}}\n\n",
        )),
    )
    .await;

    let (events, outcome) = run(&server, FakeSecrets::with("sk-test")).await;
    outcome.expect("stream succeeds");

    let completions = events
        .iter()
        .filter(|event| matches!(event, ProviderEvent::ToolCallCompleted { .. }))
        .count();
    assert_eq!(
        completions, 1,
        "a call settled by arguments.done must not be completed again by output_item.done"
    );
}

#[tokio::test]
async fn unknown_events_are_retained_diagnostically_and_never_fatal() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse(concat!(
            "event: response.some_future_event\ndata: {\"type\":\"response.some_future_event\",\"whatever\":1}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"still here\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"incomplete_details\":null,\"usage\":null}}\n\n",
        )),
    )
    .await;

    let (events, outcome) = run(&server, FakeSecrets::with("sk-test")).await;
    outcome.expect("an unknown event must not fail the stream");

    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::UnknownUpstream { .. })),
        "an unknown event must be retained, not silently dropped"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, ProviderEvent::TextDelta { text } if text == "still here")
        ),
        "decoding must continue past an unknown event"
    );
}

#[tokio::test]
async fn a_disconnect_after_partial_output_is_reported_and_never_retried() {
    let server = MockServer::start().await;
    // A stream that produces output then simply ends — no terminal event.
    mount(
        &server,
        sse("event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"half an ans\"}\n\n"),
    )
    .await;

    let (events, outcome) = run(&server, FakeSecrets::with("sk-test")).await;

    assert_eq!(
        outcome
            .expect_err("a truncated stream is an error")
            .reason_code(),
        ReasonCode::ProviderProtocol
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::TextDelta { .. })),
        "output produced before the disconnect is real"
    );

    // Exactly one request: no reconnect of a generation POST.
    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(
        requests.len(),
        1,
        "a stream that produced output must never be replayed"
    );
}

#[tokio::test]
async fn cancellation_stops_the_stream_and_reports_cancelled() {
    let server = MockServer::start().await;
    mount(
        &server,
        sse(concat!(
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"one\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"incomplete_details\":null,\"usage\":null}}\n\n",
        ))
        .set_delay(Duration::from_secs(30)),
    )
    .await;

    let provider = OpenAiProvider::new(FakeSecrets::with("sk-test")).with_base_url(server.uri());
    let (tx, _rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();

    let task = tokio::spawn({
        let cancel = cancel.clone();
        async move { provider.stream(request(), tx, cancel).await }
    });

    // Cancel while the response is still pending.
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();

    let outcome = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("cancellation must not hang")
        .expect("task");

    assert_eq!(
        outcome.expect_err("cancelled").reason_code(),
        ReasonCode::Cancelled
    );
}
