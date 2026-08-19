//! Project instructions and standards-compliant Agent Skills (plan Phase 5).
//!
//! This module loads advisory knowledge. It deliberately owns no policy gate:
//! future machine-readable project gates can be another context input without
//! pretending prose is enforceable. Skill scripts remain resources and can run
//! only through the ordinary `run_command` tool policy.

mod activate;
mod extensions;
pub mod external_agent;
mod frontmatter;
pub mod harness;
mod instructions;
mod load_extension;
mod personas;
pub mod plugins;
pub mod prompts;
pub mod self_docs;
mod skills;
pub mod soul;

use std::path::PathBuf;
use std::sync::Arc;

use etcetera::app_strategy::{AppStrategy, AppStrategyArgs, choose_native_strategy};

use crate::context::extensions::ExtensionCatalog;
use crate::context::skills::SkillCatalog;
use crate::core::context::{ContextDiagnostic, ExtensionSummary, SkillSummary};
use crate::core::tool::Tool;

pub use crate::core::context::SkillScope;
pub(crate) use load_extension::TOOL_NAME as LOAD_EXTENSION_TOOL;

const APPLICATION: &str = "smed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryLimits {
    pub max_skill_directories: usize,
    pub max_skill_file_bytes: usize,
    pub max_instruction_bytes: usize,
    pub max_resources_per_skill: usize,
}

impl Default for DiscoveryLimits {
    fn default() -> Self {
        Self {
            max_skill_directories: 256,
            max_skill_file_bytes: 256 * 1024,
            max_instruction_bytes: 512 * 1024,
            max_resources_per_skill: 256,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub project_root: PathBuf,
    pub working_directory: PathBuf,
    pub user_native_skills: PathBuf,
    pub user_agent_skills: PathBuf,
    /// The user's config directory, parent of `prompts/` .
    pub user_config: PathBuf,
    pub limits: DiscoveryLimits,
}

impl DiscoveryConfig {
    pub fn for_workspace(workspace: PathBuf) -> Result<Self, ContextError> {
        let home = etcetera::home_dir().map_err(|error| ContextError::Paths {
            detail: error.to_string(),
        })?;
        let strategy = choose_native_strategy(AppStrategyArgs {
            top_level_domain: String::new(),
            author: String::new(),
            app_name: APPLICATION.to_owned(),
        })
        .map_err(|error| ContextError::Paths {
            detail: error.to_string(),
        })?;
        Ok(Self {
            project_root: workspace.clone(),
            working_directory: workspace,
            user_native_skills: strategy.config_dir().join("skills"),
            user_agent_skills: home.join(".agents/skills"),
            user_config: strategy.config_dir(),
            limits: DiscoveryLimits::default(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("project context paths could not be resolved: {detail}")]
    Paths { detail: String },
    #[error("project instructions could not be discovered: {detail}")]
    Instructions { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivatedSkill {
    pub name: String,
    pub project: bool,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ProjectContext {
    project_root: Option<PathBuf>,
    /// smed's identity and the user profile. Inert prose in
    /// the stable prompt prefix; grants nothing, gates nothing.
    soul: Arc<Vec<soul::SoulDocument>>,
    /// Role-bound personas, overlaid on the Soul for the active route
    /// . Also inert prose; carried here, resolved by name at
    /// prompt assembly from the route the runtime selected.
    personas: Arc<personas::PersonaCatalog>,
    instructions: Arc<Vec<instructions::InstructionDocument>>,
    catalog: Arc<SkillCatalog>,
    skill_summaries: Arc<Vec<SkillSummary>>,
    /// Discovered but not yet loaded. Held so `/reload` can
    /// report what appeared or vanished; the load act that makes one callable
    /// is separate and lands in the runtime.
    extensions: Arc<ExtensionCatalog>,
    extension_summaries: Arc<Vec<ExtensionSummary>>,
    plugins: Arc<plugins::PluginCatalog>,
    plugin_summaries: Arc<Vec<crate::core::plugin::PluginSummary>>,
    external_agents: Arc<external_agent::ExternalAgentCatalog>,
    external_agent_summaries: Arc<Vec<external_agent::ExternalAgentSummary>>,
    prompts: Arc<prompts::PromptCatalog>,
    diagnostics: Arc<Vec<ContextDiagnostic>>,
    /// The configuration this context was discovered from, retained so
    /// [`reload`](Self::reload) re-reads exactly the same locations rather
    /// than re-deriving them and quietly scanning somewhere else.
    config: Option<Box<DiscoveryConfig>>,
}

impl ProjectContext {
    pub fn discover(config: DiscoveryConfig) -> Result<Self, ContextError> {
        let retained = config.clone();
        let project = config
            .project_root
            .canonicalize()
            .map_err(|error| ContextError::Paths {
                detail: format!(
                    "cannot canonicalize {}: {error}",
                    config.project_root.display()
                ),
            })?;
        let (instructions, mut diagnostics) = instructions::discover(
            &project,
            &config.working_directory,
            config.limits.max_instruction_bytes,
        )
        .map_err(|detail| ContextError::Instructions { detail })?;
        // smed's Soul and the user profile, discovered from the same locations
        // every session, so identity is not a per-project surprise.
        let (soul, soul_diagnostics) = soul::discover(
            &project,
            &config.user_config,
            config.limits.max_instruction_bytes,
        );
        diagnostics.extend(soul_diagnostics);
        let personas = personas::PersonaCatalog::discover(
            personas::roots(Some(&project), &config.user_config),
            &mut diagnostics,
        );
        let config_dir = crate::core::paths::resolve_workspace_config_dir(&project);
        let roots = vec![
            (
                config_dir.join("skills"),
                SkillScope::Project,
                Some(project.clone()),
            ),
            (
                project.join(".agents/skills"),
                SkillScope::Project,
                Some(project.clone()),
            ),
            (config.user_native_skills, SkillScope::User, None),
            (config.user_agent_skills, SkillScope::User, None),
        ];
        let catalog = SkillCatalog::discover(roots, config.limits, &mut diagnostics);
        let skill_summaries = Arc::new(catalog.summaries().to_vec());
        // Extensions are mjolnr-specific, so they live under workspace config only —
        // unlike skills, which also honour the cross-tool `.agents/` convention.
        let extension_roots = vec![(
            config_dir.join("extensions"),
            SkillScope::Project,
            Some(project.clone()),
        )];
        let extension_catalog =
            ExtensionCatalog::discover(extension_roots, config.limits, &mut diagnostics);
        let extension_summaries = Arc::new(extension_catalog.summaries().to_vec());
        let plugin_roots = vec![
            (
                config_dir.join("plugins"),
                SkillScope::Project,
                Some(project.clone()),
            ),
            (config.user_config.join("plugins"), SkillScope::User, None),
        ];
        let plugin_catalog =
            plugins::PluginCatalog::discover(plugin_roots, config.limits, &mut diagnostics);
        let plugin_summaries = Arc::new(plugin_catalog.list().to_vec());
        let ea_roots = vec![(
            config_dir.join("external-agent"),
            SkillScope::Project,
            Some(project.clone()),
        )];
        let external_agent_catalog = external_agent::ExternalAgentCatalog::discover(
            ea_roots,
            config.limits,
            &mut diagnostics,
        );
        let external_agent_summaries = Arc::new(external_agent_catalog.list().to_vec());
        let prompt_catalog = prompts::PromptCatalog::discover(
            prompts::roots(Some(&project), &config.user_config),
            &mut diagnostics,
        );
        Ok(Self {
            project_root: Some(project),
            soul: Arc::new(soul),
            personas: Arc::new(personas),
            instructions: Arc::new(instructions),
            catalog: Arc::new(catalog),
            skill_summaries,
            extensions: Arc::new(extension_catalog),
            extension_summaries,
            plugins: Arc::new(plugin_catalog),
            plugin_summaries,
            external_agents: Arc::new(external_agent_catalog),
            external_agent_summaries,
            prompts: Arc::new(prompt_catalog),
            diagnostics: Arc::new(diagnostics),
            config: Some(Box::new(retained)),
        })
    }

    #[must_use]
    pub fn empty() -> Self {
        Self {
            project_root: None,
            soul: Arc::new(Vec::new()),
            personas: Arc::new(personas::PersonaCatalog::default()),
            instructions: Arc::new(Vec::new()),
            catalog: Arc::new(SkillCatalog::default()),
            skill_summaries: Arc::new(Vec::new()),
            extensions: Arc::new(ExtensionCatalog::default()),
            extension_summaries: Arc::new(Vec::new()),
            plugins: Arc::new(plugins::PluginCatalog::default()),
            plugin_summaries: Arc::new(Vec::new()),
            external_agents: Arc::new(external_agent::ExternalAgentCatalog::default()),
            external_agent_summaries: Arc::new(Vec::new()),
            prompts: Arc::new(prompts::PromptCatalog::default()),
            diagnostics: Arc::new(Vec::new()),
            config: None,
        }
    }

    #[must_use]
    pub fn plugins(&self) -> &plugins::PluginCatalog {
        &self.plugins
    }

    #[must_use]
    pub fn plugin_summaries_arc(&self) -> Arc<Vec<crate::core::plugin::PluginSummary>> {
        Arc::clone(&self.plugin_summaries)
    }

    #[must_use]
    pub fn external_agents(&self) -> &external_agent::ExternalAgentCatalog {
        &self.external_agents
    }

    #[must_use]
    pub fn external_agent_summaries_arc(&self) -> Arc<Vec<external_agent::ExternalAgentSummary>> {
        Arc::clone(&self.external_agent_summaries)
    }

    #[must_use]
    pub fn skills(&self) -> &[SkillSummary] {
        &self.skill_summaries
    }

    /// Discovered personas as a client renders them.
    pub(crate) fn persona_summaries(&self) -> Arc<Vec<crate::core::context::PersonaSummary>> {
        Arc::new(self.personas.summaries())
    }

    /// The loaded Soul/profile files, labelled by kind and scope, for `/soul`
    /// to show what identity is in effect. A view of the
    /// record, not a control surface.
    pub(crate) fn soul_files(&self) -> Arc<Vec<String>> {
        Arc::new(
            self.soul
                .iter()
                .map(|document| {
                    format!(
                        "{} ({}) — {}",
                        document.kind.filename(),
                        document.scope.label(),
                        document.path.display()
                    )
                })
                .collect(),
        )
    }

    /// Prompt templates discovered for this project.
    #[must_use]
    pub fn prompts(&self) -> &prompts::PromptCatalog {
        &self.prompts
    }

    /// Re-read every discovered resource from disk.
    ///
    /// Re-runs the *same* discovery this context was built from, so a reload
    /// can add, change, or remove a skill or template but can never widen
    /// where smed looks. A context with no configuration — the empty one —
    /// reloads to itself, which is the honest answer for "there was no
    /// project to re-read".
    ///
    /// # Errors
    /// Whatever the original discovery would have failed with.
    pub fn reload(&self) -> Result<Self, ContextError> {
        match self.config.as_deref() {
            Some(config) => Self::discover(config.clone()),
            None => Ok(Self::empty()),
        }
    }

    /// The names of skills and templates that differ between two contexts.
    ///
    /// Reported by a reload so the acknowledgement says what changed rather
    /// than only that a reload happened.
    #[must_use]
    pub fn changes_since(&self, previous: &Self) -> Vec<String> {
        let mut changes = Vec::new();
        let before: std::collections::BTreeSet<&str> = previous
            .skill_summaries
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        let after: std::collections::BTreeSet<&str> = self
            .skill_summaries
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        changes.extend(
            after
                .difference(&before)
                .map(|name| format!("+skill {name}")),
        );
        changes.extend(
            before
                .difference(&after)
                .map(|name| format!("-skill {name}")),
        );

        let before: std::collections::BTreeSet<&str> = previous
            .prompts
            .templates()
            .iter()
            .map(|template| template.name.as_str())
            .collect();
        let after: std::collections::BTreeSet<&str> = self
            .prompts
            .templates()
            .iter()
            .map(|template| template.name.as_str())
            .collect();
        changes.extend(
            after
                .difference(&before)
                .map(|name| format!("+template {name}")),
        );
        changes.extend(
            before
                .difference(&after)
                .map(|name| format!("-template {name}")),
        );

        let before: std::collections::BTreeSet<&str> = previous
            .extension_summaries
            .iter()
            .map(|extension| extension.name.as_str())
            .collect();
        let after: std::collections::BTreeSet<&str> = self
            .extension_summaries
            .iter()
            .map(|extension| extension.name.as_str())
            .collect();
        changes.extend(
            after
                .difference(&before)
                .map(|name| format!("+extension {name}")),
        );
        changes.extend(
            before
                .difference(&after)
                .map(|name| format!("-extension {name}")),
        );

        // Identity edits. Self-evolution is an ordinary
        // Write-gated edit to a Soul, profile, or persona file; a reload is
        // where it becomes legible, so an in-place change is reported, not only
        // an add or remove — and by content, because refining a file smed
        // already wrote changes neither its name nor its path.
        changes.extend(diff_by_content(
            &soul_index(previous),
            &soul_index(self),
            "identity",
        ));
        changes.extend(diff_by_content(
            &previous.personas.digest().into_iter().collect(),
            &self.personas.digest().into_iter().collect(),
            "persona",
        ));
        changes
    }

    /// Extensions discovered for this project, before any are loaded
    /// .
    #[must_use]
    pub fn extensions(&self) -> &[ExtensionSummary] {
        &self.extension_summaries
    }

    /// The extension catalog, for the runtime's load act to resolve a name into
    /// a definition and its trust requirement.
    pub(crate) fn extension_catalog(&self) -> Arc<ExtensionCatalog> {
        Arc::clone(&self.extensions)
    }

    /// Discovered extension summaries as a client renders them.
    pub(crate) fn extension_summaries_arc(&self) -> Arc<Vec<ExtensionSummary>> {
        Arc::clone(&self.extension_summaries)
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[ContextDiagnostic] {
        &self.diagnostics
    }

    /// Assemble the system prompt, overlaying `active_persona` when a route
    /// named one and it resolves to a discovered persona file.
    ///
    /// The persona is a voice overlay on the Soul, not a replacement: it appears
    /// immediately after `<agent_soul>` so it colours the identity without
    /// displacing it. A named persona that is not on disk overlays nothing —
    /// the route runs the bare Soul rather than a hallucinated voice.
    #[must_use]
    pub fn system_prompt(&self, base: &str, active_persona: Option<&str>) -> String {
        let mut prompt = base.to_owned();
        // Identity comes first, right after the base prompt: it is the most
        // stable text (it changes rarely), so leading with it keeps the
        // provider's cacheable prefix long, and it is voice, not instruction —
        // it colours everything the model reads after it.
        if !self.soul.is_empty() {
            prompt.push_str("\n\n<agent_soul>\nSmed's own identity and the person it works for. Voice and preference only: it grants no capability, and every action it inspires still crosses the normal policy gate. You may refine these files yourself by writing them through the ordinary file-write gate — the change is diffable, reversible, and takes effect on the next /reload. More specific files appear later.\n");
            for document in self.soul.iter() {
                let tag = document.kind.tag();
                prompt.push('<');
                prompt.push_str(tag);
                prompt.push_str(" scope=\"");
                prompt.push_str(document.scope.label());
                prompt.push_str("\" path=\"");
                prompt.push_str(&xml(&document.path.display().to_string()));
                prompt.push_str("\">\n");
                prompt.push_str(&xml(&document.content));
                prompt.push_str("\n</");
                prompt.push_str(tag);
                prompt.push_str(">\n");
            }
            prompt.push_str("</agent_soul>");
        }
        // The persona overlays the Soul for the role the active route fills.
        if let Some(persona) = active_persona.and_then(|name| self.personas.overlay(name)) {
            prompt.push_str("\n\n<persona name=\"");
            prompt.push_str(&xml(&persona.name));
            prompt.push_str("\">\nThe voice for the role this route fills. Preference only; it grants no capability.\n");
            prompt.push_str(&xml(&persona.body));
            prompt.push_str("\n</persona>");
        }
        if !self.instructions.is_empty() {
            prompt.push_str("\n\n<project_instructions>\nAGENTS.md is canonical advisory context. CLAUDE.md is additional context; where they conflict, follow AGENTS.md. More specific files appear later.\n");
            for document in self.instructions.iter() {
                let role = if document.canonical {
                    "canonical"
                } else {
                    "additional"
                };
                prompt.push_str("\n<instruction_file role=\"");
                prompt.push_str(role);
                prompt.push_str("\" path=\"");
                prompt.push_str(&xml(&document.path.display().to_string()));
                prompt.push_str("\">\n");
                prompt.push_str(&xml(&document.content));
                prompt.push_str("\n</instruction_file>\n");
            }
            prompt.push_str("</project_instructions>");
        }
        if !self.skills().is_empty() {
            prompt.push_str("\n\n<available_skills>\nUse activate_skill when a task matches a description. Discovery metadata is not an execution grant.\n");
            for skill in self.skills() {
                prompt.push_str("<skill scope=\"");
                prompt.push_str(skill.scope.label());
                prompt.push_str("\"><name>");
                prompt.push_str(&xml(&skill.name));
                prompt.push_str("</name><description>");
                prompt.push_str(&xml(&skill.description));
                prompt.push_str("</description><location>");
                prompt.push_str(&xml(&skill.location));
                prompt.push_str("</location></skill>\n");
            }
            prompt.push_str("</available_skills>");
        }
        // Last, and deliberately just a list of paths: smed's own contracts,
        // for when the task is extending smed.
        if let Some(section) = self_docs::prompt_section(self.project_root.as_deref()) {
            prompt.push_str(&section);
        }
        prompt
    }

    pub(crate) fn activation_tool(&self) -> Option<Arc<dyn Tool>> {
        let project_root = self.project_root.clone()?;
        (!self.skills().is_empty()).then(|| {
            Arc::new(activate::ActivateSkill::new(
                Arc::clone(&self.catalog),
                project_root,
            )) as Arc<dyn Tool>
        })
    }

    /// The model-facing tool that proposes loading a discovered extension, when
    /// any exist. Absent otherwise, so the model is never
    /// offered a load it cannot make.
    pub(crate) fn extension_loader_tool(&self) -> Option<Arc<dyn Tool>> {
        let project_root = self.project_root.clone()?;
        (!self.extensions().is_empty()).then(|| {
            Arc::new(load_extension::LoadExtension::new(
                Arc::clone(&self.extensions),
                project_root,
            )) as Arc<dyn Tool>
        })
    }

    pub(crate) fn skills_arc(&self) -> Arc<Vec<SkillSummary>> {
        Arc::clone(&self.skill_summaries)
    }

    /// Prompt templates as a client renders them.
    pub(crate) fn prompt_summaries(&self) -> Arc<Vec<crate::core::context::PromptSummary>> {
        Arc::new(
            self.prompts
                .templates()
                .iter()
                .map(|template| crate::core::context::PromptSummary {
                    name: template.name.clone(),
                    description: template.description.clone(),
                    argument_hint: template.argument_hint.clone(),
                    scope: template.scope,
                })
                .collect(),
        )
    }

    /// Expand a template by name into the text a user message should carry.
    ///
    /// `None` when no such template exists — the caller reports that rather
    /// than sending the raw slash command to the model as if it were prose.
    #[must_use]
    pub fn expand_prompt(&self, name: &str, arguments: &str) -> Option<String> {
        let template = self.prompts.get(name)?;
        let arguments = prompts::split_arguments(arguments);
        Some(prompts::expand(&template.body, &arguments))
    }

    pub(crate) fn diagnostics_arc(&self) -> Arc<Vec<ContextDiagnostic>> {
        Arc::clone(&self.diagnostics)
    }

    pub fn activate_for_test(
        &self,
        name: &str,
    ) -> Result<ActivatedSkill, (crate::core::error::ReasonCode, String)> {
        self.catalog.activate(name)
    }
}

impl Default for ProjectContext {
    fn default() -> Self {
        Self::empty()
    }
}

pub(super) fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The Soul/profile files keyed by a stable label, mapped to their content, for
/// [`ProjectContext::changes_since`] to detect an identity edit by content.
fn soul_index(context: &ProjectContext) -> std::collections::BTreeMap<String, String> {
    context
        .soul
        .iter()
        .map(|document| {
            (
                format!("{} ({})", document.kind.filename(), document.scope.label()),
                document.content.clone(),
            )
        })
        .collect()
}

/// Report `+noun`, `-noun`, and `~noun` (changed) between two keyed content
/// maps, in name order. Unlike the by-name diffs above, this catches an
/// in-place edit — the same key with different content — which is exactly what
/// a self-evolution edit to an existing identity file looks like.
fn diff_by_content(
    before: &std::collections::BTreeMap<String, String>,
    after: &std::collections::BTreeMap<String, String>,
    noun: &str,
) -> Vec<String> {
    let mut changes = Vec::new();
    for (key, content) in after {
        match before.get(key) {
            None => changes.push(format!("+{noun} {key}")),
            Some(previous) if previous != content => changes.push(format!("~{noun} {key}")),
            Some(_) => {}
        }
    }
    for key in before.keys() {
        if !after.contains_key(key) {
            changes.push(format!("-{noun} {key}"));
        }
    }
    changes
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "AGENTS.md §7: tests may panic freely")]
mod tests {
    use super::soul::{SoulDocument, SoulKind};
    use super::*;
    use crate::core::context::SkillScope;
    use std::path::PathBuf;

    #[test]
    fn the_soul_injects_before_project_instructions() {
        let context = ProjectContext {
            soul: Arc::new(vec![SoulDocument {
                path: PathBuf::from("SOUL.md"),
                kind: SoulKind::Soul,
                scope: SkillScope::User,
                content: "I speak plainly.".to_owned(),
            }]),
            instructions: Arc::new(vec![instructions::InstructionDocument {
                path: PathBuf::from("AGENTS.md"),
                canonical: true,
                content: "Follow the standards.".to_owned(),
            }]),
            ..ProjectContext::empty()
        };

        let prompt = context.system_prompt("BASE", None);
        let soul_at = prompt.find("<agent_soul>").expect("soul section present");
        let instructions_at = prompt
            .find("<project_instructions>")
            .expect("instructions section present");
        assert!(
            soul_at < instructions_at,
            "identity leads the stable prefix, ahead of project instructions"
        );
        assert!(prompt.contains("I speak plainly."));
    }

    #[test]
    fn no_soul_means_no_soul_section() {
        let prompt = ProjectContext::empty().system_prompt("BASE", None);
        assert!(
            !prompt.contains("<agent_soul>"),
            "a session with no Soul file emits no identity section"
        );
    }

    #[test]
    fn an_active_persona_overlays_after_the_soul() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".mjolnr").join("personas");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plan.md"), "Weigh the trade-offs aloud.").unwrap();
        std::fs::write(temp.path().join(".mjolnr").join("SOUL.md"), "I am smed.").unwrap();

        let config = DiscoveryConfig::for_workspace(temp.path().to_path_buf()).unwrap();
        let context = ProjectContext::discover(config).expect("discover");

        // No persona named: the Soul stands alone.
        let bare = context.system_prompt("BASE", None);
        assert!(bare.contains("<agent_soul>"));
        assert!(!bare.contains("<persona"));

        // Naming the route's persona overlays it, after the Soul.
        let dressed = context.system_prompt("BASE", Some("plan"));
        let soul_at = dressed.find("<agent_soul>").expect("soul");
        let persona_at = dressed.find("<persona name=\"plan\">").expect("persona");
        assert!(soul_at < persona_at, "the persona overlays after the Soul");
        assert!(dressed.contains("Weigh the trade-offs aloud."));

        // A persona name with no file overlays nothing rather than inventing one.
        let missing = context.system_prompt("BASE", Some("absent"));
        assert!(!missing.contains("<persona"));
    }

    #[test]
    fn a_reload_reports_a_self_evolution_identity_edit() {
        let temp = tempfile::tempdir().unwrap();
        let smed = temp.path().join(".mjolnr");
        std::fs::create_dir_all(smed.join("personas")).unwrap();
        std::fs::write(smed.join("SOUL.md"), "I am terse.").unwrap();

        let config = DiscoveryConfig::for_workspace(temp.path().to_path_buf()).unwrap();
        let before = ProjectContext::discover(config).expect("discover");

        // smed "evolves": it edits its Soul in place and authors a new persona
        // — the writes a Write-gated tool call would have made.
        std::fs::write(
            smed.join("SOUL.md"),
            "I am terse, and I explain my reasoning.",
        )
        .unwrap();
        std::fs::write(smed.join("personas").join("mentor.md"), "Teach as you go.").unwrap();

        let after = before.reload().expect("reload");
        let changes = after.changes_since(&before);
        assert!(
            changes
                .iter()
                .any(|change| change.starts_with("~identity SOUL.md")),
            "an in-place Soul edit is reported by reload: {changes:?}"
        );
        assert!(
            changes.iter().any(|change| change == "+persona mentor"),
            "a newly authored persona is reported by reload: {changes:?}"
        );
    }
}
