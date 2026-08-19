//! Provider adapters implementing the [`Provider`](crate::core::provider::Provider) trait.
//!
//! Each bespoke adapter owns its wire types, parser, auth, tool-call assembly,
//! error mapping, usage extraction, and capability declaration. The five MVP
//! providers are deliberately not unified behind a shared "OpenAI-compatible"
//! shape : they use three tool-call models, two auth placements,
//! and two transports, and pretending otherwise would be a lie that costs
//! correctness. [`openai_compat`] (Phase 16) is the converse case: endpoints
//! that genuinely share the chat-completions wire and differ only in URL,
//! credential, and defaults, described as data instead of duplicated code.
//!
//! Real adapters arrive in Phases 2, 6, and 7. [`fake`] exists so the runtime,
//! store, and TUI can be built and tested against the same trait with no
//! network and no credentials.

pub mod anthropic;
pub mod fake;
pub mod forge;
pub mod gemini;
pub mod gemini_cli;
pub mod ollama;
pub mod openai;
pub mod openai_codex;
pub mod openai_compat;
pub mod openrouter;
pub(crate) mod quota;
