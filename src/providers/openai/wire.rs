//! OpenAI Responses wire types.
//!
//! Written against the official OpenAPI 3.1 specification
//! (`github.com/openai/openai-openapi`, MIT), read directly rather than through
//! an SDK —  forbids an unofficial OpenAI Rust SDK dependency, and
//! `docs/provider-contract.md` §1 records what was confirmed.
//!
//! These types are **deliberately partial**. smed deserialises the fields it
//! acts on and ignores the rest: `serde` skips unknown fields by default, which
//! is exactly right here. The provider adds fields continuously, and a struct
//! that insisted on knowing all of them would break every time it did.

use serde::{Deserialize, Serialize};

/// A request to `POST /v1/responses`.
///
/// No `Debug` derive on anything carrying a credential: the key never enters
/// this type at all — it goes on the `Authorization` header inside the adapter.
#[derive(Debug, Serialize)]
pub struct CreateResponse {
    pub model: String,
    pub input: Vec<InputItem>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
}

/// One item of conversation input.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum InputItem {
    #[serde(rename = "message")]
    Message {
        role: String,
        content: MessageContent,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: String },
}

/// A message's content: bare text, or parts when an image rides along.
///
/// Untagged so a text-only message serialises to exactly the string it always
/// did. The Responses API accepts both, and keeping the common shape byte-stable
/// means the existing contract fixtures keep testing what they were written to
/// test.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

/// Confirmed against current documentation 2026-07-25
/// (`provider-contract.md` §5.5).
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "input_text")]
    Text { text: String },
    #[serde(rename = "input_image")]
    Image {
        /// A `data:` URI. smed holds the bytes and never hands the provider a
        /// URL to fetch, which would be an outbound request nothing reviewed.
        image_url: String,
        detail: &'static str,
    },
}

#[derive(Debug, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub strict: bool,
}

/// A streaming event.
///
/// Tagged by the `type` field, whose values are the literal wire strings
/// confirmed in the spec. `#[serde(other)]` on [`Unknown`](Self::Unknown) is
/// load-bearing: Anthropic documents that clients must tolerate new event types,
/// OpenAI ships them continuously, and  requires retaining them
/// diagnostically rather than failing.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    #[serde(rename = "response.created")]
    Created,

    #[serde(rename = "response.in_progress")]
    InProgress,

    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta { delta: String },

    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta { delta: String },

    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta { delta: String },

    #[serde(rename = "response.output_text.done")]
    OutputTextDone,

    #[serde(rename = "response.output_item.added")]
    OutputItemAdded { item: OutputItem },

    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta { item_id: String, delta: String },

    /// The parse boundary for tool arguments. Accumulated fragments are only
    /// valid JSON here — earlier is a syntax error by construction.
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        item_id: String,
        /// Present in the current server-event reference, absent from older
        /// Responses streaming schemas. The item correlation remains required
        /// either way.
        call_id: Option<String>,
        name: String,
        arguments: String,
    },

    #[serde(rename = "response.output_item.done")]
    OutputItemDone { item: OutputItem },

    #[serde(rename = "response.completed")]
    Completed { response: Response },

    #[serde(rename = "response.failed")]
    Failed { response: Response },

    /// A third terminal state: the model stopped early. Neither success nor
    /// failure, and reporting it as either would misreport state.
    #[serde(rename = "response.incomplete")]
    Incomplete { response: Response },

    /// Retained, never fatal.
    #[serde(other)]
    Unknown,
}

/// An item in the response output.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum OutputItem {
    /// A tool call.
    ///
    /// Carries **both** `id` (`fc_…`) and `call_id` (`call_…`). The result must
    /// quote `call_id`; using `id` appears to work until it doesn't
    /// (`docs/provider-contract.md` §1).
    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        #[serde(default)]
        arguments: String,
    },

    #[serde(other)]
    Other,
}

/// The `Response` object carried by every terminal event.
#[derive(Debug, Deserialize)]
pub struct Response {
    pub status: Option<String>,
    pub error: Option<ResponseError>,
    pub incomplete_details: Option<IncompleteDetails>,
    pub usage: Option<ResponseUsage>,
}

/// `{ code, message }`. `code` values include `rate_limit_exceeded`, which is
/// why a rate limit can arrive mid-stream under HTTP 200.
#[derive(Debug, Deserialize)]
pub struct ResponseError {
    pub code: Option<String>,
    pub message: Option<String>,
    #[serde(alias = "resets_at", alias = "reset_at")]
    pub reset_at_unix: Option<i64>,
}

/// Why a response stopped early: `max_output_tokens` or `content_filter`.
#[derive(Debug, Deserialize)]
pub struct IncompleteDetails {
    pub reason: Option<String>,
}

/// Token usage.
///
/// The `_details` sub-objects are **breakdowns of** their parents, not additions
/// to them: `cached_tokens ⊆ input_tokens`, `reasoning_tokens ⊆ output_tokens`.
/// smed drops them — real information the MVP has no surface for — rather than
/// summing them into a wrong total.
#[derive(Debug, Deserialize)]
pub struct ResponseUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

/// The body of a non-200 response: `{ "error": { … } }`.
///
/// Note the asymmetry with [`ErrorFrameBody`]: the HTTP body wraps the error,
/// the mid-stream SSE `error` frame does not. Decoding one with the other's
/// shape fails.
#[derive(Debug, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

#[derive(Debug, Deserialize)]
pub struct ErrorBody {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub message: Option<String>,
    pub code: Option<String>,
    #[serde(alias = "resets_at", alias = "reset_at")]
    pub reset_at_unix: Option<i64>,
}

/// The payload of an `event: error` SSE frame.
///
/// The current reference places `code` and `message` at the top level. The
/// production stream has also emitted a wrapped `error` during request setup,
/// so both documented and observed shapes are decoded without retaining raw
/// provider prose.
#[derive(Debug, Deserialize)]
pub struct ErrorFrameBody {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub code: Option<String>,
    pub error: Option<ErrorBody>,
    #[serde(alias = "resets_at", alias = "reset_at")]
    pub reset_at_unix: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_events_deserialise_rather_than_failing() {
        // The provider ships new event types continuously. A decoder that
        // rejected them would break on a Tuesday for no reason.
        let json = r#"{"type":"response.some_future_thing","data":{"x":1}}"#;
        let event: StreamEvent = serde_json::from_str(json).expect("unknown event must decode");
        assert!(matches!(event, StreamEvent::Unknown));
    }

    #[test]
    fn unknown_fields_on_known_events_are_ignored() {
        // Same reason, one level down: a new field must not break a known event.
        let json = r#"{"type":"response.output_text.delta","delta":"hi","sequence_number":7,"brand_new":true}"#;
        let event: StreamEvent = serde_json::from_str(json).expect("decode");
        match event {
            StreamEvent::OutputTextDelta { delta } => assert_eq!(delta, "hi"),
            other => panic!("expected a text delta, got {other:?}"),
        }
    }

    #[test]
    fn a_function_call_keeps_both_ids_distinct() {
        let json = r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_abc","call_id":"call_xyz","name":"read_file","arguments":""}}"#;
        let event: StreamEvent = serde_json::from_str(json).expect("decode");

        match event {
            StreamEvent::OutputItemAdded {
                item:
                    OutputItem::FunctionCall {
                        id, call_id, name, ..
                    },
            } => {
                assert_eq!(id, "fc_abc");
                assert_eq!(call_id, "call_xyz");
                assert_ne!(id, call_id, "the two ids are different and both matter");
                assert_eq!(name, "read_file");
            }
            other => panic!("expected a function call, got {other:?}"),
        }
    }

    #[test]
    fn a_failed_response_carries_its_error_code() {
        let json = r#"{"type":"response.failed","response":{"status":"failed","error":{"code":"rate_limit_exceeded","message":"slow down"},"incomplete_details":null,"usage":null}}"#;
        let event: StreamEvent = serde_json::from_str(json).expect("decode");

        match event {
            StreamEvent::Failed { response } => {
                assert_eq!(response.status.as_deref(), Some("failed"));
                let error = response.error.expect("error present");
                // A rate limit arriving under HTTP 200 is the trap this exists for.
                assert_eq!(error.code.as_deref(), Some("rate_limit_exceeded"));
            }
            other => panic!("expected a failed response, got {other:?}"),
        }
    }

    #[test]
    fn an_incomplete_response_carries_why_it_stopped() {
        let json = r#"{"type":"response.incomplete","response":{"status":"incomplete","error":null,"incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#;
        let event: StreamEvent = serde_json::from_str(json).expect("decode");

        match event {
            StreamEvent::Incomplete { response } => {
                assert_eq!(
                    response
                        .incomplete_details
                        .and_then(|details| details.reason)
                        .as_deref(),
                    Some("max_output_tokens")
                );
            }
            other => panic!("expected an incomplete response, got {other:?}"),
        }
    }

    #[test]
    fn usage_details_are_not_added_to_their_parents() {
        // cached_tokens ⊆ input_tokens and reasoning_tokens ⊆ output_tokens.
        // smed ignores the breakdowns; this pins that they are not summed in.
        let json = r#"{"input_tokens":100,"input_tokens_details":{"cached_tokens":80},"output_tokens":50,"output_tokens_details":{"reasoning_tokens":40},"total_tokens":150}"#;
        let usage: ResponseUsage = serde_json::from_str(json).expect("decode");

        assert_eq!(
            usage.input_tokens, 100,
            "cached tokens must not be added on"
        );
        assert_eq!(
            usage.output_tokens, 50,
            "reasoning tokens must not be added on"
        );
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn the_two_error_envelopes_have_different_shapes() {
        // Non-200 body: wrapped.
        let wrapped = r#"{"error":{"type":"invalid_request_error","message":"bad key","param":null,"code":"invalid_api_key"}}"#;
        let response: ErrorResponse = serde_json::from_str(wrapped).expect("wrapped decode");
        assert_eq!(response.error.code.as_deref(), Some("invalid_api_key"));

        // Mid-stream `error` frame: bare.
        let bare = r#"{"type":"error","message":"boom","param":null,"code":"server_error","sequence_number":1}"#;
        let frame: ErrorFrameBody = serde_json::from_str(bare).expect("bare decode");
        assert_eq!(frame.kind.as_deref(), Some("error"));
        assert_eq!(frame.code.as_deref(), Some("server_error"));

        let observed = r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad schema","code":"invalid_function_parameters"}}"#;
        let frame: ErrorFrameBody = serde_json::from_str(observed).expect("observed decode");
        assert_eq!(
            frame.error.and_then(|error| error.code).as_deref(),
            Some("invalid_function_parameters")
        );

        // And the shapes are genuinely not interchangeable.
        assert!(
            serde_json::from_str::<ErrorResponse>(bare).is_err(),
            "a bare error must not decode as a wrapped one"
        );
    }

    #[test]
    fn a_request_serialises_without_optional_fields() {
        let request = CreateResponse {
            model: "gpt-4o-mini".to_owned(),
            input: vec![InputItem::Message {
                role: "user".to_owned(),
                content: MessageContent::Text("hi".to_owned()),
            }],
            stream: true,
            instructions: None,
            tools: Vec::new(),
        };

        let json = serde_json::to_string(&request).expect("serialise");
        assert!(json.contains(r#""stream":true"#));
        assert!(json.contains(r#""type":"message""#));
        assert!(
            !json.contains("instructions"),
            "None must be omitted, not sent as null"
        );
    }
}
