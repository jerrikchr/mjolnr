//! `mjolnr init`: scaffold a starter routing config (Pillar 2).
//!
//! One reason to change: how a first routing config is offered and written.
//!
//! # Why this module may print to stdout
//!
//! Like [`super::auth`], this runs **instead of** the TUI, never alongside it,
//! so stdout is not the alternate screen. The allowance is per-module and
//! justified rather than crate-wide (`AGENTS.md` §4).
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI subcommands run instead of the TUI, so stdout is not the alternate screen"
)]

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::routing::scaffold::{self, ProviderSeed, ScaffoldFile};

/// How `mjolnr init` was invoked.
#[derive(Debug)]
pub struct InitOptions {
    /// Skip the interactive confirmation. The scriptable path; still
    /// non-destructive, because refusing to overwrite is a property of the
    /// plan, not of the prompt.
    pub assume_yes: bool,
}

/// What generating against this project came to: the files that do not yet
/// exist and would be written, and the ones that already exist and must be
/// left exactly as they are.
///
/// Shared with the guided onboarding flow (`super::onboard`) so both write
/// through one non-destructive path: an existing file is always reported and
/// left untouched, never clobbered.
#[derive(Debug)]
pub(crate) struct Plan {
    pub(crate) to_write: Vec<ScaffoldFile>,
    pub(crate) existing: Vec<PathBuf>,
}

/// Run `mjolnr init`. Returns the process exit code.
///
/// Never overwrites: a generated file whose path already exists is reported and
/// left untouched, so re-running after hand-editing a route can only add the
/// files that are still missing. The write happens only after the preview and
/// an explicit yes.
#[must_use]
pub fn run(seeds: &[ProviderSeed], project_root: &Path, options: &InitOptions) -> i32 {
    let mut files = scaffold::generate(seeds);
    if files.is_empty() {
        eprintln!(
            "no provider credential resolves — run `mjolnr auth login <provider>`, then `mjolnr init`"
        );
        return 1;
    }
    // A starting Soul, offered on the same terms as everything else here:
    // previewed, never overwriting, and a plain file the owner can edit or
    // delete. `plan_writes` drops it when one already exists.
    let (soul_path, soul_contents) = crate::context::soul::default_soul();
    files.push(ScaffoldFile {
        relative_path: soul_path,
        contents: soul_contents,
    });

    // And the governance floor, on the same terms. It ships
    // with rows and a `supervised` default rather than empty: a file whose
    // presence changes nothing until someone enumerates every model in advance
    // is not a starting point, it is homework.
    let (governance_path, governance_contents) = crate::governance::starting_file();
    files.push(ScaffoldFile {
        relative_path: governance_path,
        contents: governance_contents,
    });

    let plan = plan_writes(&files, project_root);
    print_preview(&plan);

    if plan.to_write.is_empty() {
        println!("nothing to do — every file already exists and was left untouched");
        return 0;
    }
    if !options.assume_yes && !confirm() {
        println!("aborted — nothing was written");
        return 1;
    }
    match write_all(&plan.to_write, project_root) {
        Ok(()) => {
            println!(
                "wrote {} file(s) under .mjolnr/. Edit them freely, then run `mjolnr` and try `/route` or `/role`.",
                plan.to_write.len()
            );
            0
        }
        Err(error) => {
            eprintln!("could not write the scaffold: {error}");
            1
        }
    }
}

/// Split the generated files into those to write and those already present.
///
/// Pure over the filesystem's current state — it reads to classify but writes
/// nothing — so the non-destructive guarantee can be tested without a prompt.
pub(crate) fn plan_writes(files: &[ScaffoldFile], project_root: &Path) -> Plan {
    let mut to_write = Vec::new();
    let mut existing = Vec::new();
    for file in files {
        if project_root.join(&file.relative_path).exists() {
            existing.push(file.relative_path.clone());
        } else {
            to_write.push(file.clone());
        }
    }
    Plan { to_write, existing }
}

pub(crate) fn print_preview(plan: &Plan) {
    for path in &plan.existing {
        println!("exists, leaving untouched: {}", path.display());
    }
    for file in &plan.to_write {
        println!("\n── {} ──", file.relative_path.display());
        print!("{}", file.contents);
    }
    println!();
}

pub(crate) fn confirm() -> bool {
    print!("write the file(s) above? [y/N] ");
    if std::io::stdout().flush().is_err() {
        return false;
    }
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim(), "y" | "Y" | "yes" | "YES")
}

pub(crate) fn write_all(files: &[ScaffoldFile], project_root: &Path) -> std::io::Result<()> {
    for file in files {
        let path = project_root.join(&file.relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &file.contents)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{ModelId, ProviderId};

    fn seed(provider: &str, model: &str) -> ProviderSeed {
        ProviderSeed {
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
        }
    }

    #[test]
    fn init_writes_a_parseable_scaffold_into_an_empty_project() {
        let temp = tempfile::tempdir().expect("tempdir");
        let code = run(
            &[seed("openai", "gpt-5.4")],
            temp.path(),
            &InitOptions { assume_yes: true },
        );
        assert_eq!(code, 0);
        // The generated files load back through the Phase 15 loader with the
        // default role indexed — the round trip the whole feature rests on.
        let (table, diagnostics) = crate::routing::load_dir(temp.path());
        assert!(
            diagnostics.is_empty(),
            "scaffold must load without diagnostics"
        );
        assert_eq!(table.roles.get("default"), Some(&"openai".to_owned()));
        assert_eq!(
            table.task_classes.get("default"),
            Some(&"openai".to_owned())
        );
    }

    #[test]
    fn init_lays_down_a_starting_soul_the_owner_can_read() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _ = run(
            &[seed("openai", "gpt-5.4")],
            temp.path(),
            &InitOptions { assume_yes: true },
        );
        let soul = temp.path().join(".mjolnr").join("SOUL.md");
        let contents = std::fs::read_to_string(&soul).expect("a Soul was written");
        assert!(contents.contains("# Soul"));
        assert!(
            contents.contains("Delete it and smed runs without a Soul"),
            "the file must tell its owner it is optional"
        );
    }

    #[test]
    fn an_existing_soul_is_never_overwritten() {
        let temp = tempfile::tempdir().expect("tempdir");
        let smed = temp.path().join(".mjolnr");
        std::fs::create_dir_all(&smed).expect("dir");
        std::fs::write(smed.join("SOUL.md"), "mine, hand-written\n").expect("existing soul");

        let _ = run(
            &[seed("openai", "gpt-5.4")],
            temp.path(),
            &InitOptions { assume_yes: true },
        );

        // The Soul is the file most likely to hold work nobody can regenerate.
        assert_eq!(
            std::fs::read_to_string(smed.join("SOUL.md")).expect("read"),
            "mine, hand-written\n"
        );
    }

    #[test]
    fn the_default_soul_carries_no_authority() {
        // Law 7 admits self-evolution only because identity is inert prose. A
        // shipped default that granted anything would break that at the seam
        // where every new project starts.
        let (_, contents) = crate::context::soul::default_soul();
        for authority in [
            "full-auto",
            "policy",
            "approve",
            "allowed-tools",
            "run_command",
        ] {
            assert!(
                !contents.contains(authority),
                "the default Soul must not mention {authority}: it is voice, not capability"
            );
        }
    }

    #[test]
    fn init_never_overwrites_an_existing_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let routes = temp.path().join(".mjolnr").join("routes");
        std::fs::create_dir_all(&routes).expect("mkdir");
        let route_file = routes.join("openai.yaml");
        std::fs::write(
            &route_file,
            "hops:\n  - provider: openai\n    model: hand-edited\n",
        )
        .expect("write");

        let code = run(
            &[seed("openai", "gpt-5.4")],
            temp.path(),
            &InitOptions { assume_yes: true },
        );
        assert_eq!(code, 0);
        // The hand-authored route is exactly as it was — never clobbered.
        let contents = std::fs::read_to_string(&route_file).expect("read");
        assert!(
            contents.contains("hand-edited"),
            "an existing route must survive init verbatim"
        );
    }

    #[test]
    fn no_seeds_is_an_explained_refusal_not_a_write() {
        let temp = tempfile::tempdir().expect("tempdir");
        let code = run(&[], temp.path(), &InitOptions { assume_yes: true });
        assert_eq!(code, 1);
        assert!(
            !temp.path().join(".mjolnr").exists(),
            "a refusal must not create .mjolnr/"
        );
    }

    #[test]
    fn plan_separates_missing_from_existing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let files = scaffold::generate(&[seed("openai", "gpt-5.4")]);
        // Pre-create just the routing.yaml; the route file is still missing.
        let smed = temp.path().join(".mjolnr");
        std::fs::create_dir_all(&smed).expect("mkdir");
        std::fs::write(smed.join("routing.yaml"), "task_classes: {}\n").expect("write");

        let plan = plan_writes(&files, temp.path());
        assert_eq!(plan.existing, vec![PathBuf::from(".mjolnr/routing.yaml")]);
        assert_eq!(plan.to_write.len(), 1);
        let only = plan.to_write.first().expect("one file to write");
        assert_eq!(
            only.relative_path,
            PathBuf::from(".mjolnr/routes/openai.yaml")
        );
    }
}
