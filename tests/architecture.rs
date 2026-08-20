//! Enforces the dependency direction (AGENTS.md §2.1).
//!
//! ```text
//! tui → runtime → core (traits + types)
//!                  ↑        ↑
//!             providers   tools / store / policy / context
//! ```
//!
//! mjolnr is one binary by design , so **no process or crate boundary
//! makes this true**. Without this test the rule is a comment, and a comment
//! loses to a deadline. Phase 0 shipped the rule as prose; this is the phase
//! that makes it fail the build.
//!
//! It is a source scan, not a type-level trick. That is a deliberate trade: it
//! is approximate (it reads `use` statements and paths, not the resolved module
//! graph) but it is legible, has no build-time cost, and its failure message
//! names the rule that was broken. A cleverer mechanism nobody understands would
//! be deleted the first time it was inconvenient.

#![allow(clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A module and what it is forbidden to depend on.
struct Rule {
    module: &'static str,
    forbidden: &'static [&'static str],
    because: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        module: "core",
        forbidden: &[
            "tui",
            "runtime",
            "providers",
            "store",
            "tools",
            "policy",
            "context",
            "repository",
            "integrations",
        ],
        because: "core is the base of the dependency graph: it defines contracts and may not \
                  depend on anything that implements them",
    },
    Rule {
        module: "tui",
        forbidden: &["providers", "store", "tools", "policy", "memory", "plugins"],
        because: "the TUI is a client: it renders snapshots and emits commands, and may never \
                  call a provider, touch the store, execute a tool, or reach a capability \
                  module directly — everything memory-shaped arrives via `core` snapshot types",
    },
    Rule {
        module: "providers",
        forbidden: &["tui", "store", "policy", "runtime"],
        because: "an adapter knows its wire format and nothing else — not policy, not \
                  persistence, not the UI ",
    },
    Rule {
        module: "store",
        forbidden: &["tui", "providers", "runtime", "policy"],
        because: "a store implements the EventStore port and knows nothing about who writes to it",
    },
    // Phase 4 gave the CLI real capability — it reads the database now — so the
    // line worth drawing is what it must *not* become.
    //
    // It may name a provider (`auth login openai` has to know the id it stores a
    // key under) and it must reach the store. It may not drive a runtime: that
    // would make `mjolnr <something>` a second, undocumented client of the agent
    // loop, outside every gate the TUI passes through. `main.rs` stays the only
    // place a runtime is wired.
    Rule {
        module: "cli",
        forbidden: &["tui", "runtime"],
        because: "the CLI runs *instead of* the TUI, and must not become a second client of the \
                  agent loop; main.rs is the only composition root",
    },
    Rule {
        module: "runtime",
        forbidden: &["tui"],
        because: "the runtime must not know a terminal exists; it is the reason a second client \
                  (web, mobile approval surface) is possible later without a rewrite",
    },
    Rule {
        module: "context",
        forbidden: &["tui", "runtime", "providers", "store", "policy"],
        because: "project context discovers inert instructions and skill resources; it must not own policy, persistence, provider, runtime, or UI behavior",
    },
    // The code graph  is the strictest module in the tree: it
    // takes an already-canonicalised root and returns data, so it needs nothing
    // internal at all. Keeping it that way is what makes the graph testable
    // without a workspace, a store, or a runtime — and what would let it become
    // a crate by moving the directory (AGENTS.md §2.2).
    Rule {
        module: "graph",
        forbidden: &[
            "tui",
            "runtime",
            "providers",
            "store",
            "policy",
            "tools",
            "context",
            "core",
        ],
        because: "the code graph is a leaf analysis module: source text in, structure out, with \
                  path containment left to its caller",
    },
    // The governance loader. It reads one declared file and
    // builds a table; what a tier *means* lives in `core::governance`, which
    // does not know a file exists. The rule that matters is `runtime`: a
    // loader that could reach the runtime could apply a tier as well as read
    // one, and applying is the half that has to stay at the gate.
    Rule {
        module: "governance",
        forbidden: &[
            "tui",
            "runtime",
            "providers",
            "store",
            "policy",
            "tools",
            "context",
            "routing",
        ],
        because: "the governance loader reads a declared file into a table and applies nothing: \
                  the clamp belongs to the runtime, at the door every other ceiling passes",
    },
    // The scheduler process. It drives a `Runtime` to fire
    // scheduled and webhook directives, so — unlike `cli` — it *is* allowed to
    // depend on `runtime`; main.rs wires `mjolnr triggers run` to it exactly as
    // it wires `mjolnr exec` to `headless`. What it must never become is a
    // second terminal UI.
    Rule {
        module: "triggers",
        forbidden: &["tui"],
        because: "the scheduler is a background process, never a terminal client",
    },
    // Armed ahead of the code (§D5/§D6): neither module exists yet, and the
    // scan silently passes on an absent directory — that is the point. The
    // guard is in place *before* the first `src/repository/` or
    // `src/integrations/` file lands, so the phase that introduces the module
    // cannot drift past the boundary unnoticed.
    Rule {
        module: "repository",
        forbidden: &["tui", "runtime", "providers", "store", "policy", "tools"],
        because: "the repository projection runs governed git operations and returns typed \
                  results; the runtime calls it, never the reverse, and it must not reach \
                  persistence, policy, providers, or any client",
    },
    // The D7 file producer. It is the one module outside `tools` that is
    // *allowed* to name `policy`, and that is the whole point of it: containment
    // is `policy::paths`' job and §D7 requires it rechecked immediately before
    // every read. A rule that forbade `policy` here — copied from `repository`
    // above, where it is right — would push containment into this module and
    // give the codebase a second answer to the only question that must have one.
    //
    // What it may not do is grow the other halves of a workspace. `repository`
    // is forbidden because the ignore answer arrives as an argument: a module
    // that both walked the filesystem and shelled out to git would have two
    // reasons to change (AGENTS.md §2.3), and the runtime composing the two is
    // what keeps each testable without the other.
    Rule {
        module: "workspace_files",
        forbidden: &[
            "tui",
            "runtime",
            "providers",
            "store",
            "tools",
            "repository",
            "integrations",
            "context",
        ],
        because: "the file projection performs contained filesystem reads and returns typed \
                  results; the runtime calls it, never the reverse, and it must not reach \
                  persistence, providers, git, or any client",
    },
    Rule {
        module: "integrations",
        forbidden: &["tui", "runtime", "providers", "store", "policy", "tools"],
        because: "task-source integrations fetch externally supplied data and hand it to the \
                  bridge framed as data; they must not reach the runtime, persistence, \
                  policy, providers, or any client",
    },
    // Armed ahead of the code (master implementation plan Phase 1 / ADR-0016),
    // exactly as `repository` and `integrations` were: the scan silently passes
    // on an absent directory, so the guard is in place before the first
    // `src/memory/` file lands and the phase cannot drift past the boundary
    // unnoticed.
    //
    // The memory module is a projection (Standing Law #2): `.mjolnr/data/
    // memory.db` is disposable and regenerable, and the append-only ledger is
    // truth. A memory module that could reach the runtime could act on its own
    // recall, and one that could reach `store` could write the ledger it must
    // only ever read *about* — so both are forbidden. `policy` is the one
    // deliberate omission, for the same reason as `workspace_files`: the rules
    // loader rechecks containment via `policy::paths` immediately before every
    // read, and a second containment answer would be worse than the import.
    Rule {
        module: "memory",
        forbidden: &[
            "tui",
            "runtime",
            "providers",
            "store",
            "tools",
            "repository",
            "integrations",
            "context",
        ],
        because: "the memory projection owns recall aids only (Standing Law #2): it may not \
                  reach the runtime it could otherwise act through, the ledger it must never \
                  rewrite, or any client — containment stays `policy::paths`, the one answer",
    },
    // Armed ahead of the code (ADR-0016, Phase 2): the plugin host speaks a
    // subprocess protocol and registers tools through the ordinary gate. It
    // must never reach a client, the ledger, or a provider — a plugin host
    // that could write the store could forge the evidence a completion is
    // gated on, and one that could reach `providers` would hold credentials it
    // was never granted.
    Rule {
        module: "plugins",
        forbidden: &["tui", "store", "providers"],
        because: "the plugin host (ADR-0016) governs third-party subprocesses: it may offer \
                  tools through the ordinary gate but must never reach a client, the event \
                  ledger, or a provider",
    },
    // D9 — external-agent isolation and review fence. Armed before the directory
    // exists (same pattern as `repository`/`integrations`/`memory`/`plugins`):
    // a dedicated worktree + bounded activity log feeding D3 review and D5
    // staging. Must not reach a client, the ledger, a provider, or its own
    // policy/store helpers — reuse is through the bridge and the actor.
    Rule {
        module: "external_agent",
        forbidden: &[
            "tui",
            "providers",
            "store",
            "policy",
            "repository",
            "integrations",
            "context",
        ],
        because: "external-agent isolation and review-fence code renders via the bridge; it \
                  must not call a provider, touch the store, widen policy, or name repository / \
                  integrations / context directly",
    },
];

/// Nothing outside `tui` may import `tui`. Stated separately because it is the
/// single most important edge: it is what keeps the core headless.
const TUI_IS_A_LEAF: &str = "tui";

#[test]
fn dependency_direction_holds() {
    let sources = collect_sources(&src_root());
    assert!(
        sources.len() > 5,
        "source scan found only {} files — the scan is broken, not the code",
        sources.len()
    );

    let mut violations = Vec::new();

    for (path, contents) in &sources {
        let Some(module) = top_module(path) else {
            continue;
        };

        for rule in RULES {
            if rule.module != module {
                continue;
            }

            for forbidden in rule.forbidden {
                for (line_number, line) in imports(contents) {
                    if references_module(&line, forbidden) {
                        violations.push(format!(
                            "{}:{line_number}\n    `{}` may not depend on `{}`\n    {}\n    offending line: {}",
                            path.display(),
                            rule.module,
                            forbidden,
                            rule.because,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "dependency direction violated (AGENTS.md §2.1):\n\n{}\n",
        violations.join("\n\n")
    );
}

#[test]
fn nothing_outside_tui_imports_tui() {
    let sources = collect_sources(&src_root());
    let mut violations = Vec::new();

    for (path, contents) in &sources {
        let module = top_module(path);
        if module.as_deref() == Some(TUI_IS_A_LEAF) {
            continue;
        }
        // `lib.rs` declares every module; `main.rs` is the composition root and
        // is allowed to wire the TUI to the runtime. Those are the only two.
        if is_crate_root(path) {
            continue;
        }

        for (line_number, line) in imports(contents) {
            if references_module(&line, TUI_IS_A_LEAF) {
                violations.push(format!(
                    "{}:{line_number} imports `tui`: {}",
                    path.display(),
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "`tui` must be a leaf — only main.rs may wire it (AGENTS.md §2.1):\n\n{}\n",
        violations.join("\n")
    );
}

fn check_desktop_backend_boundary(desktop_tauri_src: &Path) {
    if !desktop_tauri_src.exists() {
        return;
    }
    let sources = collect_sources(desktop_tauri_src);
    let mut violations = Vec::new();

    for (path, contents) in &sources {
        for (line_number, line) in imports(contents) {
            if line.contains("mjolnr::tui") {
                violations.push(format!(
                    "{}:{line_number} imports `tui` directly into desktop client: {}",
                    path.display(),
                    line.trim()
                ));
            }
            if line.contains("mjolnr::policy::") {
                violations.push(format!(
                    "{}:{line_number} imports policy internal gate module into desktop client: {}",
                    path.display(),
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "desktop client backend must never import forbidden internal modules (ADR 0003):\n\n{}\n",
        violations.join("\n")
    );
}

fn check_desktop_frontend_boundary(desktop_frontend_src: &Path) {
    if !desktop_frontend_src.exists() {
        return;
    }
    let mut violations = Vec::new();
    let mut stack = vec![desktop_frontend_src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(ext) = path.extension().and_then(std::ffi::OsStr::to_str)
                && (ext == "ts" || ext == "svelte")
                && let Ok(contents) = std::fs::read_to_string(&path)
                && (contents.contains("mockDispatch") || contents.contains("agentLoop"))
            {
                violations.push(format!(
                    "{}: manufactured frontend agent authority / mock dispatch forbidden",
                    path.display()
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "desktop frontend must never contain manufactured agent authority (ADR 0004):\n\n{}\n",
        violations.join("\n")
    );
}

#[test]
fn desktop_client_boundary_holds() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    check_desktop_backend_boundary(&root.join("desktop").join("src-tauri").join("src"));
    check_desktop_frontend_boundary(&root.join("desktop").join("src"));
}

/// The desktop frontend is a client. Two edges keep it one:
///
/// 1. A `.ts`/`.svelte` file may not import anything that resolves outside
///    `desktop/` — that is how the Rust source tree (`src/`) stays invisible
///    to Svelte, so the frontend can never grow a second authority surface.
/// 2. `paneforge` may only be imported inside the `ui/resizable/` wrapper, so
///    split-pane behaviour has exactly one home in the design system instead
///    of leaking ad-hoc imports through routes and surfaces.
#[test]
fn desktop_frontend_imports_stay_inside_the_client() {
    let desktop = Path::new(env!("CARGO_MANIFEST_DIR")).join("desktop");
    let frontend = desktop.join("src");
    if !frontend.exists() {
        return;
    }
    let resizable_wrapper = frontend
        .join("lib")
        .join("components")
        .join("ui")
        .join("resizable");

    let mut violations = Vec::new();
    let mut stack = vec![frontend];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(extension) = path.extension().and_then(std::ffi::OsStr::to_str) else {
                continue;
            };
            if extension != "ts" && extension != "svelte" {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (line_number, line) in contents.lines().enumerate() {
                let Some(specifier) = import_specifier(line) else {
                    continue;
                };
                if (specifier == "paneforge" || specifier.starts_with("paneforge/"))
                    && !path.starts_with(&resizable_wrapper)
                {
                    violations.push(format!(
                        "{}:{}\n    `paneforge` may only be imported inside \
                         desktop/src/lib/components/ui/resizable/\n    offending line: {}",
                        path.display(),
                        line_number + 1,
                        line.trim()
                    ));
                    continue;
                }
                if !specifier.starts_with('.') {
                    continue; // package or `$lib` alias import: stays a client
                }
                let Some(parent) = path.parent() else {
                    continue;
                };
                let resolved = lexically_normalize(&parent.join(specifier));
                if !resolved.starts_with(&desktop) {
                    violations.push(format!(
                        "{}:{}\n    frontend import escapes `desktop/`; the Rust source \
                         tree must stay invisible to the client\n    offending line: {}",
                        path.display(),
                        line_number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "desktop frontend escaped its client boundary (ADR 0006):\n\n{}\n",
        violations.join("\n\n")
    );
}

/// The specifier of a static import/export line, if the line is one. Matches
/// `import ... from 'x'`, `export ... from 'x'`, side-effect `import 'x'`,
/// and the `} from 'x'` tail of a multi-line import.
fn import_specifier(line: &str) -> Option<&str> {
    let line = line.trim();
    if !line.starts_with("import ") && !line.starts_with("export ") && !line.starts_with("} from") {
        return None;
    }
    let quote_start = line.find(['\'', '"'])?;
    let quote = line.as_bytes()[quote_start];
    let rest = &line[quote_start + 1..];
    let quote_end = rest.find(quote as char)?;
    Some(&rest[..quote_end])
}

/// Resolve `.` and `..` without touching the filesystem. Symlinks are
/// irrelevant here: the rule is about where source text points, not where the
/// filesystem would land.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// The integrated-workspace DTOs (`core::client::workspace`) are the wire
/// contract: `core::client` defines them, `runtime::client_bridge` projects
/// them, and nothing else may depend on them. In particular the D5
/// `repository`, D6 `integrations`, and D7 `workspace_files` modules are
/// runtime-side infrastructure that must project through the bridge, never
/// reach into the client contract directly (ADR 0006).
#[test]
fn workspace_dtos_are_consumed_only_by_the_bridge() {
    let root = src_root();
    let sources = collect_sources(&root);
    let mut violations = Vec::new();

    for (path, contents) in &sources {
        let in_contract_module = path.starts_with(root.join("core").join("client"));
        let in_bridge = path.starts_with(root.join("runtime").join("client_bridge"));
        if in_contract_module || in_bridge || is_crate_root(path) {
            continue;
        }

        for (line_number, line) in imports(contents) {
            if line.contains("crate::core::client::workspace") {
                violations.push(format!(
                    "{}:{line_number}\n    only `core::client` (definition) and \
                     `runtime::client_bridge` (projection) may reference the workspace \
                     DTOs; everything else projects through the bridge\n    offending \
                     line: {}",
                    path.display(),
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "workspace DTOs escaped the bridge boundary (ADR 0006):\n\n{}\n",
        violations.join("\n\n")
    );
}

/// The board wire contract (`core::client::board`, Phase E5 step 3) follows
/// the same boundary as the workspace DTOs: `core::client` defines it, the
/// bridge projects it, and nothing else may depend on it. `core::frontier`
/// stays the pure core model — it must never import the client contract, or
/// the bridge would stop being the single place provenance crosses.
#[test]
fn board_dtos_are_consumed_only_by_the_bridge() {
    let root = src_root();
    let sources = collect_sources(&root);
    let mut violations = Vec::new();

    for (path, contents) in &sources {
        let in_contract_module = path.starts_with(root.join("core").join("client"));
        let in_bridge = path.starts_with(root.join("runtime").join("client_bridge"));
        if in_contract_module || in_bridge || is_crate_root(path) {
            continue;
        }

        for (line_number, line) in imports(contents) {
            if line.contains("crate::core::client::board") {
                violations.push(format!(
                    "{}:{line_number}\n    only `core::client` (definition) and \
                     `runtime::client_bridge` (projection) may reference the board \
                     DTOs; core surfaces the pure frontier, never the wire shape\n    \
                     offending line: {}",
                    path.display(),
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "board DTOs escaped the bridge boundary (E5 step 3):\n\n{}\n",
        violations.join("\n\n")
    );
}

/// The client bridge is a thin translation layer: it maps `ClientCommand` to
/// `MjolnrCommand` and projects DTOs, full stop. Reuse of runtime machinery
/// (the subagent worktree engine, the repository projection) happens in the
/// actor's `handle_command`, never in the bridge — a bridge that can reach
/// runtime internals is a second authority surface (Phase D2 review).
#[test]
fn bridge_stays_a_thin_translation_layer() {
    let root = src_root().join("runtime").join("client_bridge");
    if !root.exists() {
        return;
    }
    let sources = collect_sources(&root);
    let mut violations = Vec::new();

    for (path, contents) in &sources {
        for (line_number, line) in imports(contents) {
            for forbidden in [
                "crate::runtime::subagent",
                "crate::repository",
                "crate::workspace_files",
            ] {
                if line.contains(forbidden) {
                    violations.push(format!(
                        "{}:{line_number}\n    the bridge must not reference `{forbidden}`; \
                         runtime reuse belongs in the actor's handle_command\n    \
                         offending line: {}",
                        path.display(),
                        line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "client bridge reached into runtime internals (Phase D2 boundary):\n\n{}\n",
        violations.join("\n\n")
    );
}

/// A dispatch path that panics is fail-open by crash, not fail-closed by
/// refusal (AGENTS.md §3: "Missing capability → refuse before the request").
/// `todo!`, `unimplemented!`, and `panic!` are therefore forbidden in
/// `src/runtime/` production code. Test code may still use them: the scan
/// skips files that *are* test modules (`tests.rs`, `*_tests.rs`) and stops
/// at each remaining file's `#[cfg(test)]` marker — the codebase keeps inline
/// test modules at the end of the file, and this test is what keeps that
/// convention true.
#[test]
fn runtime_dispatch_never_panics_on_unimplemented() {
    let sources = collect_sources(&src_root().join("runtime"));
    let mut violations = Vec::new();

    for (path, contents) in &sources {
        let is_test_file = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name == "tests.rs" || name.ends_with("_tests.rs"));
        if is_test_file {
            continue;
        }
        for (line_number, line) in contents.lines().enumerate() {
            if line.contains("#[cfg(test)]") {
                break;
            }
            let code = strip_comment(line);
            for macro_name in ["todo!(", "unimplemented!(", "panic!("] {
                if code.contains(macro_name) {
                    violations.push(format!(
                        "{}:{}\n    `{macro_name}` in production runtime code is a crash, not \
                         a refusal; return a typed error instead\n    offending line: {}",
                        path.display(),
                        line_number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "runtime dispatch must refuse, never panic (AGENTS.md §3):\n\n{}\n",
        violations.join("\n\n")
    );
}

/// Grouped `crate::{…}` imports are banned.
///
/// Not style policing: `use crate::{providers, core};` never contains the string
/// `crate::providers`, so no substring scan can see it. Banning the one shape is
/// simpler and more robust than parsing brace groups, and it is sufficient —
/// every other form spells the top-level module literally (`crate::core::{A, B}`
/// still contains `crate::core`).
///
/// `super::super::` is banned for the same reason: it reaches the crate root
/// while spelling no module name the scanner can match.
#[test]
fn no_brace_grouped_crate_imports() {
    let sources = collect_sources(&src_root());
    let mut violations = Vec::new();

    for (path, contents) in &sources {
        for (line_number, line) in imports(contents) {
            if line.contains("crate::{") {
                violations.push(format!(
                    "{}:{line_number} groups a crate-level import, hiding the module name from \
                     the dependency scan: {}",
                    path.display(),
                    line.trim()
                ));
            }
            if line.contains("super::super::") {
                violations.push(format!(
                    "{}:{line_number} reaches the crate root via `super::super::`, bypassing the \
                     dependency scan: {}",
                    path.display(),
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "imports must name their module so the dependency scan can see them \
         (AGENTS.md §2.1):\n\n{}\n",
        violations.join("\n")
    );
}

/// The scan must actually be capable of failing. A rule checker that cannot
/// detect a violation is worse than none: it reports success forever.
///
/// The cases below are deliberately the forms someone would reach for under a
/// deadline, not only the ones the codebase currently uses. The alias case is
/// here because it was a real bypass: the original scanner matched only
/// `crate::providers::` and `crate::providers;`, so an alias import passed.
#[test]
fn the_scanner_detects_a_violation() {
    let hostile = "use crate::providers::fake::FakeProvider;";
    assert!(
        references_module(hostile, "providers"),
        "the scanner cannot see a plain `use crate::providers` import"
    );

    assert!(
        references_module("    use crate::store::memory::InMemoryEventStore;", "store"),
        "the scanner cannot see an indented import"
    );

    assert!(
        references_module("let x: crate::tui::reducer::ViewState;", "tui"),
        "the scanner cannot see a fully-qualified path outside a use statement"
    );

    // Evasion forms. Each of these was, or would have been, a real bypass.
    assert!(
        references_module("use crate::providers as evasion_probe;", "providers"),
        "an alias import must not walk past the scanner"
    );
    assert!(
        references_module("use crate::providers;", "providers"),
        "a bare module import must be caught"
    );
    assert!(
        references_module("pub use crate::tui as ui;", "tui"),
        "a re-export must be caught"
    );

    // Lookalikes that must NOT fire.
    assert!(
        !references_module("use crate::core::store::EventStore;", "store"),
        "`core::store` is the port, not the `store` module — the scanner must not confuse them"
    );
    assert!(
        !references_module("// the store module is not imported here", "store"),
        "a comment must not count as a dependency"
    );
    assert!(
        !references_module("use crate::core::provider::Provider;", "providers"),
        "`core::provider` is the trait, not the `providers` module"
    );
    assert!(
        !references_module("use crate::store_helpers::Thing;", "store"),
        "`store_helpers` is not `store`: the match must end on a word boundary"
    );
}

fn src_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn is_crate_root(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(std::ffi::OsStr::to_str),
        Some("lib.rs" | "main.rs")
    ) && path.parent() == Some(src_root().as_path())
}

/// The top-level module a source file belongs to, e.g. `src/core/message.rs` →
/// `core`.
fn top_module(path: &Path) -> Option<String> {
    let root = src_root();
    let relative = path.strip_prefix(&root).ok()?;
    let first = relative.components().next()?;
    let name = first.as_os_str().to_str()?;
    if is_rust_source(Path::new(name)) {
        return None; // a file directly in src/, i.e. lib.rs or main.rs
    }
    Some(name.to_owned())
}

/// Lines that could express a dependency, with 1-based line numbers.
///
/// Comments are stripped first so prose about a module is not mistaken for a
/// dependency on it.
fn imports(contents: &str) -> Vec<(usize, String)> {
    contents
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, strip_comment(line)))
        .filter(|(_, line)| line.contains("crate::") || line.contains("use "))
        .collect()
}

fn strip_comment(line: &str) -> String {
    match line.find("//") {
        Some(position) => line[..position].to_owned(),
        None => line.to_owned(),
    }
}

/// Whether a line references `crate::<module>`.
///
/// Matching on the `crate::` prefix is what prevents `core::store` (the port)
/// being mistaken for `store` (the implementation) — a distinction this codebase
/// deliberately maintains, so the scanner has to as well.
///
/// The match ends on any non-identifier character rather than a fixed `::` or
/// `;`. An earlier version accepted only those two, so `use crate::providers as
/// alias;` walked straight past it. The forms that matter are the ones someone
/// reaches for under a deadline, not the ones the codebase already uses.
///
/// Grouped imports (`use crate::{providers, core};`) hide the module name from
/// any substring search. Rather than parsing them, [`no_brace_grouped_crate_imports`]
/// bans that one shape outright — it is the *only* way to name a top-level
/// module without spelling `crate::<module>`, since a deeper group like
/// `crate::core::{A, B}` still contains `crate::core` literally.
fn references_module(line: &str, module: &str) -> bool {
    let needle = format!("crate::{module}");
    line.match_indices(&needle).any(|(index, _)| {
        let tail = line.get(index + needle.len()..).unwrap_or("");
        // End of line, `::`, `;`, ` as x`, `,`, `}` all count. `_` and
        // alphanumerics do not, so `crate::store_helpers` is not `crate::store`.
        tail.chars()
            .next()
            .is_none_or(|character| !is_identifier_char(character))
    })
}

fn is_identifier_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Case-insensitive: macOS filesystems are case-insensitive by default, so a
/// `Foo.RS` would be a real source file the scan must not skip.
fn is_rust_source(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
}

fn collect_sources(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut sources = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if is_rust_source(&path)
                && let Ok(contents) = std::fs::read_to_string(&path)
            {
                sources.insert(path, contents);
            }
        }
    }

    sources
}

#[test]
fn no_stale_brand_in_user_facing_strings() {
    let sources = collect_sources(&src_root());
    let mut violations = Vec::new();

    for (path, contents) in &sources {
        if path.ends_with("src/core/paths.rs") {
            continue;
        }

        for (line_number, line) in contents.lines().enumerate() {
            let line_number = line_number + 1;
            if line.contains("smed") || line.contains("Smed") || line.contains("SMED") {
                violations.push(format!("{}:{line_number}: {}", path.display(), line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "stale brand name 'smed' found in source tree (ADR-0018):\n\n{}\n",
        violations.join("\n")
    );
}
