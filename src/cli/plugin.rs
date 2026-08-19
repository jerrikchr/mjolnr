//! `mjolnr plugin` — third-party plugin scaffolding (Phase 6, ADR-0016).
//!
//! Generates a fail-closed `smed-plugin.yaml` manifest under
//! `.mjolnr/plugins/<name>.yaml` plus an optional language starter that speaks
//! JSON-RPC 2.0 over stdio. Reuses `super::init::plan_writes` / `print_preview`
//! / `confirm` / `write_all` so the usual never-overwrites guarantee holds.
//! Runs instead of the TUI, so stdout is not the alternate screen.

#![allow(
    clippy::cmp_owned,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI subcommands run instead of the TUI, so stdout is not the alternate screen"
)]

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};

use crate::core::plugin::{PLUGIN_PROTOCOL_VERSION, PluginManifest};
use crate::routing::scaffold::ScaffoldFile;

#[derive(Debug, Subcommand)]
pub enum PluginCommand {
    /// Scaffold a new third-party plugin manifest.
    ///
    /// Writes `.mjolnr/plugins/<name>.yaml`. With --template, also writes a
    /// minimal JSON-RPC stdio host under `plugins/<name>/`.
    Create(CreateArgs),
    /// List discovered plugins (reads .mjolnr/plugins/ and user config dir).
    List,
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Plugin name, e.g. acme.deploy (lowercase, digits, dots, hyphens, underscores).
    pub name: String,

    /// Language starter to generate alongside the manifest.
    #[arg(long, value_enum)]
    pub template: Option<PluginTemplate>,

    /// Skip the preview confirmation. Still never overwrites an existing file.
    #[arg(long)]
    pub yes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PluginTemplate {
    Node,
    Rust,
    Python,
}

#[must_use]
fn scaffold_manifest(name: &str, template: Option<PluginTemplate>) -> String {
    let (program, args) = match template {
        Some(PluginTemplate::Node) | None => {
            ("node".to_owned(), format!("plugins/{name}/index.js"))
        }
        Some(PluginTemplate::Rust) => (format!("plugins/{name}/plugin-{name}"), String::new()),
        Some(PluginTemplate::Python) => ("python3".to_owned(), format!("plugins/{name}/main.py")),
    };

    let run_args = if args.is_empty() {
        "  arguments: []\n".to_owned()
    } else {
        format!("  arguments: [\"{args}\"]\n")
    };

    format!(
        "name: {name}\n\
         version: 0.1.0\n\
         publisher: local\n\
         description: A mjolnr plugin — replace this description\n\
         protocol_version: {PLUGIN_PROTOCOL_VERSION}\n\
         run:\n  program: {program}\n\
         {run_args}\
         tools: []\n\
         hooks: []\n\
         required_credentials: []\n\
         views: []\n",
    )
}

fn node_starter(name: &str) -> String {
    format!(
        r#"#!/usr/bin/env node
"use strict";
const readline = require("readline");
const rl = readline.createInterface({{ input: process.stdin, terminal: false }});
function reply(id, result) {{
  process.stdout.write(JSON.stringify({{ jsonrpc: "2.0", id, result }}) + "\n");
}}
function error(id, code, message) {{
  process.stdout.write(JSON.stringify({{ jsonrpc: "2.0", id, error: {{ code, message }} }}) + "\n");
}}
rl.on("line", async (line) => {{
  line = line.trim();
  if (!line) return;
  let req;
  try {{ req = JSON.parse(line); }} catch {{ return; }}
  const id = req.id;
  const method = req.method;
  // TODO: handle tools declared in .mjolnr/plugins/{name}.yaml
  try {{
    if (method === "initialize") {{
      reply(id, {{ status: "ready", protocol_version: 1, plugin: "{name}" }});
    }} else if (method === "session_start") {{
      reply(id, {{ annotations: ["{name} ready"], notices: [] }});
    }} else if (method === "call_tool") {{
      error(id, -32601, `unknown tool ${{req.params && req.params.name}}`);
    }} else if (method === "shutdown") {{
      reply(id, {{ status: "shutting down" }});
      process.exit(0);
    }} else {{
      error(id, -32601, `unknown method ${{method}}`);
    }}
  }} catch (e) {{
    error(id, -32603, e && e.message ? e.message : String(e));
  }}
}});
"#
    )
}

fn node_package(name: &str) -> String {
    format!(
        r#"{{
  "name": "{name}",
  "version": "0.1.0",
  "private": true,
  "type": "commonjs"
}}
"#
    )
}

fn rust_starter(name: &str) -> String {
    format!(
        r#"use std::io::{{BufRead, Write}};

fn main() {{
    let stdin = std::io::stdin();
    for line in stdin.lock().lines().flatten() {{
        let line = line.trim().to_owned();
        if line.is_empty() {{
            continue;
        }}
        let req: serde_json::Value = match serde_json::from_str(&line) {{
            Ok(v) => v,
            Err(_) => continue,
        }};
        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let result = match method {{
            "initialize" => serde_json::json!({{"status": "ready", "protocol_version": 1, "plugin": "{name}"}}),
            "session_start" => serde_json::json!({{"annotations": ["{name} ready"], "notices": []}}),
            "shutdown" => {{
                respond(&id, serde_json::json!({{"status": "shutting down"}}), None);
                break;
            }},
            _ => {{
                respond(&id, serde_json::Value::Null, Some(( -32601, format!("unknown method {{method}}"))));
                continue;
            }},
        }};
        respond(&id, result, None);
    }}
}}

fn respond(id: &serde_json::Value, result: serde_json::Value, err: Option<(i64, String)>) {{
    let msg = if let Some((code, message)) = err {{
        serde_json::json!({{"jsonrpc": "2.0", "id": id, "error": {{"code": code, "message": message}}}})
    }} else {{
        serde_json::json!({{"jsonrpc": "2.0", "id": id, "result": result}})
    }};
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{{}}", msg);
    let _ = out.flush();
}}
"#
    )
}

fn rust_cargo(name: &str) -> String {
    format!(
        r#"[package]
name = "plugin-{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
serde_json = "1"
serde = {{ version = "1", features = ["derive"] }}
"#
    )
}

fn python_starter(name: &str) -> String {
    format!(
        r#"import sys, json

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
    except Exception:
        continue
    rid = req.get("id")
    method = req.get("method")

    def reply(result):
        sys.stdout.write(json.dumps({{"jsonrpc": "2.0", "id": rid, "result": result}}) + "\n")
        sys.stdout.flush()

    def err(code, message):
        sys.stdout.write(json.dumps({{"jsonrpc": "2.0", "id": rid, "error": {{"code": code, "message": message}}}}) + "\n")
        sys.stdout.flush()

    try:
        if method == "initialize":
            reply({{"status": "ready", "protocol_version": 1, "plugin": "{name}"}})
        elif method == "session_start":
            reply({{"annotations": ["{name} ready"], "notices": []}})
        elif method == "shutdown":
            reply({{"status": "shutting down"}})
            sys.exit(0)
        else:
            err(-32601, f"unknown method {{method}}")
    except Exception as e:
        err(-32603, str(e))
"#
    )
}

fn python_pyproject(name: &str) -> String {
    format!(
        r#"[project]
name = "plugin-{name}"
version = "0.1.0"
requires-python = ">=3.10"
dependencies = []

[build-system]
requires = ["setuptools"]
"#
    )
}

pub fn scaffold_plugin(
    name: &str,
    template: Option<PluginTemplate>,
) -> Result<Vec<ScaffoldFile>, String> {
    let manifest_yaml = scaffold_manifest(name, template);
    PluginManifest::parse(&manifest_yaml)
        .map_err(|e| format!("generated manifest failed validation: {e}"))?;

    let mut files = vec![ScaffoldFile {
        relative_path: PathBuf::from(format!(".mjolnr/plugins/{name}.yaml")),
        contents: manifest_yaml,
    }];

    match template {
        None => {}
        Some(PluginTemplate::Node) => {
            files.push(ScaffoldFile {
                relative_path: PathBuf::from(format!("plugins/{name}/package.json")),
                contents: node_package(name),
            });
            files.push(ScaffoldFile {
                relative_path: PathBuf::from(format!("plugins/{name}/index.js")),
                contents: node_starter(name),
            });
        }
        Some(PluginTemplate::Rust) => {
            files.push(ScaffoldFile {
                relative_path: PathBuf::from(format!("plugins/{name}/Cargo.toml")),
                contents: rust_cargo(name),
            });
            files.push(ScaffoldFile {
                relative_path: PathBuf::from(format!("plugins/{name}/src/main.rs")),
                contents: rust_starter(name),
            });
        }
        Some(PluginTemplate::Python) => {
            files.push(ScaffoldFile {
                relative_path: PathBuf::from(format!("plugins/{name}/pyproject.toml")),
                contents: python_pyproject(name),
            });
            files.push(ScaffoldFile {
                relative_path: PathBuf::from(format!("plugins/{name}/main.py")),
                contents: python_starter(name),
            });
        }
    }

    Ok(files)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "CreateArgs is a clap value type"
)]
#[must_use]
pub fn run_create(args: CreateArgs, project_root: &Path) -> i32 {
    let files = match scaffold_plugin(&args.name, args.template) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("mjolnr plugin create: {error}");
            return 1;
        }
    };

    let plan = super::init::plan_writes(&files, project_root);
    super::init::print_preview(&plan);

    if plan.to_write.is_empty() {
        println!("nothing to do — every file already exists and was left untouched");
        return 0;
    }
    if !args.yes && !super::init::confirm() {
        println!("aborted — nothing was written");
        return 1;
    }
    match super::init::write_all(&plan.to_write, project_root) {
        Ok(()) => {
            println!(
                "wrote {} file(s). Edit .mjolnr/plugins/{}.yaml freely; discovery reads .mjolnr/plugins/*.yaml.",
                plan.to_write.len(),
                args.name
            );
            0
        }
        Err(error) => {
            eprintln!("could not write the scaffold: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_manifest_validates_for_every_template() {
        for template in [
            None,
            Some(PluginTemplate::Node),
            Some(PluginTemplate::Rust),
            Some(PluginTemplate::Python),
        ] {
            let files = scaffold_plugin("acme.example", template).expect("scaffold");
            let manifest_file = files.first().expect("manifest");
            assert_eq!(
                manifest_file.relative_path,
                PathBuf::from(".mjolnr/plugins/acme.example.yaml")
            );
            PluginManifest::parse(&manifest_file.contents).expect("valid manifest");
        }
    }

    #[test]
    fn scaffold_rejects_an_invalid_name() {
        let err = scaffold_plugin("Bad Name!", None).expect_err("must refuse");
        assert!(err.contains("plugin") || err.contains("name") || err.contains("Bad"));
    }

    #[test]
    fn plan_writes_never_overwrites_an_existing_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let existing = temp.path().join(".mjolnr/plugins/acme.example.yaml");
        std::fs::create_dir_all(existing.parent().expect("parent")).expect("mkdir");
        std::fs::write(&existing, "hand-edited: true\n").expect("write");

        let files = scaffold_plugin("acme.example", None).expect("scaffold");
        let plan = crate::cli::init::plan_writes(&files, temp.path());
        assert!(
            plan.existing
                .contains(&PathBuf::from(".mjolnr/plugins/acme.example.yaml"))
        );
        assert_eq!(
            std::fs::read_to_string(&existing).expect("read"),
            "hand-edited: true\n"
        );
    }

    #[test]
    fn node_template_emits_package_and_index() {
        let files = scaffold_plugin("acme.example", Some(PluginTemplate::Node)).expect("scaffold");
        assert_eq!(files.len(), 3);
        assert!(
            files
                .iter()
                .any(|f| f.relative_path == PathBuf::from("plugins/acme.example/package.json"))
        );
        assert!(
            files
                .iter()
                .any(|f| f.relative_path == PathBuf::from("plugins/acme.example/index.js"))
        );
    }

    #[test]
    fn rust_template_emits_cargo_and_main() {
        let files = scaffold_plugin("acme.example", Some(PluginTemplate::Rust)).expect("scaffold");
        assert_eq!(files.len(), 3);
        assert!(
            files
                .iter()
                .any(|f| f.relative_path == PathBuf::from("plugins/acme.example/Cargo.toml"))
        );
        assert!(
            files
                .iter()
                .any(|f| f.relative_path == PathBuf::from("plugins/acme.example/src/main.rs"))
        );
    }

    #[test]
    fn python_template_emits_pyproject_and_main() {
        let files =
            scaffold_plugin("acme.example", Some(PluginTemplate::Python)).expect("scaffold");
        assert_eq!(files.len(), 3);
        assert!(
            files
                .iter()
                .any(|f| f.relative_path == PathBuf::from("plugins/acme.example/pyproject.toml"))
        );
        assert!(
            files
                .iter()
                .any(|f| f.relative_path == PathBuf::from("plugins/acme.example/main.py"))
        );
    }
}
