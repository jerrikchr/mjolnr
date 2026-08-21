//! The slash-command registry.
//!
//! One list, so the autocomplete menu, the help panel, and the dispatcher in
//! [`app`](crate::tui::app) cannot disagree about what exists. A command the
//! dispatcher handles but the menu omits is undiscoverable; a command the menu
//! offers but the dispatcher rejects is a lie.

use crate::core::runtime::RuntimeSnapshot;
use crate::tui::reducer::ViewState;

/// A slash command as the menu presents it.
pub struct SlashCommand {
    pub name: &'static str,
    /// Extra spellings that dispatch identically. Listed here so the menu can
    /// match them, but never shown as separate rows.
    pub aliases: &'static [&'static str],
    pub summary: &'static str,
    /// Argument shape, when the command takes one.
    pub hint: Option<&'static str>,
    /// Live state for this command, rendered beside it. Follows oh-my-pi's
    /// `getTuiAutocompleteDescription`: the menu doubles as a status readout, so
    /// checking the current model does not require opening anything.
    pub state: fn(&ViewState) -> Option<String>,
}

impl std::fmt::Debug for SlashCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlashCommand")
            .field("name", &self.name)
            .field("aliases", &self.aliases)
            .field("summary", &self.summary)
            .field("hint", &self.hint)
            .finish_non_exhaustive()
    }
}

pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "/help",
        aliases: &["/keymap"],
        summary: "keyboard shortcuts and interaction reference",
        hint: None,
        state: |_| None,
    },
    SlashCommand {
        name: "/model",
        aliases: &["/models"],
        summary: "switch provider and model",
        hint: Some("[provider model]"),
        state: |view| {
            let provider = view.snapshot.provider.as_ref()?;
            let model = view.snapshot.model.as_ref()?;
            Some(format!("{}/{}", provider.as_str(), model.as_str()))
        },
    },
    SlashCommand {
        name: "/auth",
        aliases: &["/login", "/provider"],
        summary: "provider credentials · select to register",
        hint: None,
        state: |view| {
            let total = view.snapshot.providers.len();
            if total == 0 {
                return None;
            }
            let ready = view
                .snapshot
                .providers
                .iter()
                .filter(|provider| {
                    provider.state == crate::core::runtime::ProviderConnectionState::Connected
                })
                .count();
            Some(format!("{ready}/{total} providers connected"))
        },
    },
    SlashCommand {
        name: "/policy",
        aliases: &[],
        summary: "select policy mode",
        hint: Some("[mode]"),
        state: |view| Some(view.snapshot.policy.label().to_owned()),
    },
    SlashCommand {
        name: "/envelope",
        aliases: &[],
        summary: "pre-authorise a bounded population of subagent spawns",
        hint: Some("[children] [ceiling] [turns] | off"),
        state: |view| {
            view.snapshot.envelope.as_ref().map_or_else(
                || Some("none armed".to_owned()),
                |active| {
                    Some(format!(
                        "{} of {} children left · {}",
                        active.children_remaining(),
                        active.envelope.max_children,
                        active.envelope.ceiling.label()
                    ))
                },
            )
        },
    },
    SlashCommand {
        name: "/route",
        aliases: &[],
        summary: "attach a configured route by name",
        hint: Some("[route]"),
        state: |view| {
            view.snapshot.route.as_ref().map_or_else(
                || Some(format!("{} configured", view.snapshot.routes.len())),
                |route| Some(format!("on {}", route.route)),
            )
        },
    },
    SlashCommand {
        name: "/role",
        aliases: &[],
        summary: "attach the route a project tags with a role",
        hint: Some("[role]"),
        state: |view| {
            let roles: std::collections::BTreeSet<&str> = view
                .snapshot
                .routes
                .iter()
                .flat_map(|choice| choice.roles.iter().map(String::as_str))
                .collect();
            Some(format!("{} role(s)", roles.len()))
        },
    },
    SlashCommand {
        name: "/persona",
        aliases: &[],
        summary: "overlay a persona's voice, or clear it with `off`",
        hint: Some("[name|off]"),
        state: |view| {
            Some(view.snapshot.active_persona.as_ref().map_or_else(
                || format!("{} available", view.snapshot.personas.len()),
                |name| format!("on {name}"),
            ))
        },
    },
    SlashCommand {
        name: "/soul",
        aliases: &[],
        summary: "show the identity and profile files in effect",
        hint: None,
        state: |view| Some(format!("{} file(s)", view.snapshot.souls.len())),
    },
    SlashCommand {
        name: "/usage",
        aliases: &[],
        summary: "quota basis and spend",
        hint: None,
        state: |view| {
            let usage = view.snapshot.usage;
            Some(format!(
                "{} in / {} out",
                usage.input_tokens, usage.output_tokens
            ))
        },
    },
    SlashCommand {
        name: "/theme",
        aliases: &["/palette"],
        summary: "select visual theme and view detected colour depth",
        hint: Some("[theme]"),
        state: |_| {
            Some(format!(
                "{} · {}",
                crate::tui::theme::active_theme_id().name(),
                crate::tui::theme::detected_color_depth().label()
            ))
        },
    },
    SlashCommand {
        name: "/skills",
        aliases: &[],
        summary: "available skills",
        hint: None,
        state: |view| Some(format!("{} available", view.snapshot.skills.len())),
    },
    SlashCommand {
        name: "/mcp",
        aliases: &[],
        summary: "governed external tool servers",
        hint: None,
        state: |view| Some(format!("{} server(s)", view.snapshot.mcp_servers.len())),
    },
    SlashCommand {
        name: "/triggers",
        aliases: &[],
        summary: "configured triggers and last outcome",
        hint: None,
        state: |view| Some(format!("{} configured", view.snapshot.triggers.len())),
    },
    SlashCommand {
        name: "/memory",
        aliases: &[],
        summary: "workspace memory, rules snapshot & episodes",
        hint: None,
        state: |view| {
            Some(format!(
                "{} rules · {} facts",
                view.snapshot.memory.rules_count,
                view.snapshot
                    .memory
                    .facts_count
                    .map_or("unknown".to_owned(), |count| count.to_string())
            ))
        },
    },
    SlashCommand {
        name: "/plugins",
        aliases: &[],
        summary: "inspect third-party capability plugins (.mjolnr/plugins/)",
        hint: None,
        state: |view| Some(format!("{} discovered", view.snapshot.plugins.len())),
    },
    SlashCommand {
        name: "/external",
        aliases: &["/agents"],
        summary: "external CLI agents in dedicated worktrees [EXTERNAL · UNVERIFIED]",
        hint: None,
        state: |view| {
            let n = view.snapshot.external_agents.len();
            Some(if n == 0 {
                "none".to_owned()
            } else {
                format!("{n} agent(s)")
            })
        },
    },
    SlashCommand {
        name: "/handoff",
        aliases: &[],
        summary: "checkpoint & live swap model (/handoff <role|model>)",
        hint: Some("<role|model>"),
        state: |_| None,
    },
    SlashCommand {
        name: "/council",
        aliases: &[],
        summary: "convene multi-model deliberation (/council <question>|plan <path>)",
        hint: Some("<question>|plan <path>"),
        state: |_| None,
    },
    SlashCommand {
        name: "/config",
        aliases: &[],
        summary: "open interactive settings & configuration surface",
        hint: None,
        state: |_| None,
    },
    SlashCommand {
        name: "/plan",
        aliases: &[],
        summary: "start an owner interview or open plan mode",
        hint: Some("<goal>"),
        state: |_| None,
    },
    SlashCommand {
        name: "/tree",
        aliases: &[],
        summary: "explore session history and turns",
        hint: None,
        state: |_| None,
    },
    SlashCommand {
        name: "/fork",
        aliases: &[],
        summary: "start a new session from a turn in /tree, leaving this one alone",
        hint: Some("<turn number from /tree>"),
        state: |_| None,
    },
    SlashCommand {
        name: "/clone",
        aliases: &[],
        summary: "duplicate this branch into a new session",
        hint: None,
        state: |_| None,
    },
    SlashCommand {
        name: "/reload",
        aliases: &[],
        summary: "re-read skills, prompts, and project instructions",
        hint: None,
        state: |view| {
            let prompts = view.snapshot.prompts.len();
            let skills = view.snapshot.skills.len();
            Some(format!("{skills} skill(s) · {prompts} template(s)"))
        },
    },
    SlashCommand {
        name: "/discover",
        aliases: &[],
        summary: "scan the open repository and write a bounded OKF bundle",
        hint: None,
        state: |view| {
            view.snapshot.last_discovery.as_ref().map_or_else(
                || Some("not run".to_owned()),
                |report| {
                    Some(format!(
                        "{} files · {}",
                        report.source_files,
                        report.bundle_path.display()
                    ))
                },
            )
        },
    },
    SlashCommand {
        name: "/load-extension",
        aliases: &[],
        summary: "load a discovered extension, making its tool callable this session",
        hint: Some("<extension name>"),
        state: |view| {
            let count = view.snapshot.extensions.len();
            Some(format!("{count} discovered"))
        },
    },
    SlashCommand {
        name: "/leave",
        aliases: &[],
        summary: "release the open session (resumable later, not terminal)",
        hint: None,
        state: |view| view.snapshot.session.as_ref().map(|s| format!("session {}", &s.to_string()[..8.min(s.to_string().len())])),
    },
    SlashCommand {
        name: "/end",
        aliases: &[],
        summary: "end the open session permanently (cannot be resumed)",
        hint: None,
        state: |view| view.snapshot.session.as_ref().map(|s| format!("session {}", &s.to_string()[..8.min(s.to_string().len())])),
    },
    SlashCommand {
        name: "/reclaim",
        aliases: &[],
        summary: "break a write lease a crashed process left behind",
        hint: Some("<session id prefix>"),
        state: |view| {
            let stale: Vec<_> = view.snapshot.sessions.iter().filter(|s| s.leased).collect();
            Some(format!("{} stale lease(s)", stale.len()))
        },
    },
];

/// One row of the command menu: a built-in or a discovered prompt template.
///
/// Owned rather than `&'static` because templates come from disk. Built-ins
/// keep their static text and simply borrow into this shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuEntry {
    pub name: String,
    pub summary: String,
    pub hint: Option<String>,
    pub state: Option<String>,
    pub template: bool,
}

/// Whether `name` (with leading slash) is a built-in command or one of its
/// aliases.
///
/// The collision rule in one place: built-ins own their names, and a template
/// that picks one is shadowed rather than allowed to redefine `/model`.
#[must_use]
pub(crate) fn is_builtin(name: &str) -> bool {
    COMMANDS
        .iter()
        .any(|command| command.name == name || command.aliases.contains(&name))
}

/// Commands matching what has been typed so far.
///
/// Returns everything for a bare `/`, so the menu is a catalogue before it is a
/// filter. An empty result means the text matches nothing, which the caller
/// renders rather than silently closing the menu.
pub(crate) fn matching(input: &str) -> Vec<&'static SlashCommand> {
    let needle = input.trim().to_lowercase();
    COMMANDS
        .iter()
        .filter(|command| {
            std::iter::once(command.name)
                .chain(command.aliases.iter().copied())
                .any(|name| name.starts_with(&needle))
        })
        .collect()
}

/// The menu rows for what has been typed: built-ins first, then templates.
///
/// Built-ins lead because they always resolve; a template that shadows one is
/// dropped here, and the discovery layer already reported the collision, so
/// the menu never offers a row that would dispatch to something else.
pub(crate) fn menu_entries(input: &str, view: &ViewState) -> Vec<MenuEntry> {
    let needle = input.trim().to_lowercase();
    let mut entries: Vec<MenuEntry> = matching(input)
        .into_iter()
        .map(|command| MenuEntry {
            name: command.name.to_owned(),
            summary: command.summary.to_owned(),
            hint: command.hint.map(str::to_owned),
            state: (command.state)(view),
            template: false,
        })
        .collect();
    for template in view.snapshot.prompts.iter() {
        let name = format!("/{}", template.name);
        if is_builtin(&name) || !name.starts_with(&needle) {
            continue;
        }
        entries.push(MenuEntry {
            name,
            summary: template.description.clone(),
            hint: template.argument_hint.clone(),
            state: Some("prompt template".to_owned()),
            template: true,
        });
    }
    entries
}

/// Whether the composer text should open the command menu.
///
/// Only while typing the command word itself: once there is a space the user is
/// writing arguments, and a menu covering the transcript then is in the way.
pub(crate) fn menu_applies(composer: &str) -> bool {
    composer.starts_with('/') && !composer.trim_end().contains(' ')
}

/// Which field a `/route` or `/role` invocation fills. Both dispatch the same
/// [`AttachRoute`](crate::core::command::MjolnrCommand::AttachRoute); they
/// differ only in whether the argument is an explicit route name or a role the
/// project tags a route with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteBy {
    Name,
    Role,
}

/// What a `/route` or `/role` command resolves to.
///
/// Decided purely from the snapshot so the choice is unit-testable without a
/// runtime — and, more importantly, so an unknown name becomes a *stated*
/// notice instead of a silent `AttachRoute` no-op. `attach_route` deliberately
/// does nothing when a name does not resolve (the Phase 15 contract); a user
/// who typed `/role plan` and saw nothing happen would be told a lie by
/// silence (§1.3), so the client validates against the offered choices first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RoutePlan {
    Attach {
        route: Option<String>,
        role: Option<String>,
    },
    Notice(String),
}

/// Longest list of names spelled out in a notice before it summarises the tail.
const MAX_LISTED_NAMES: usize = 12;

/// Decide what `/route <argument>` or `/role <argument>` should do.
pub(crate) fn plan_route_command(
    by: RouteBy,
    argument: &str,
    snapshot: &RuntimeSnapshot,
) -> RoutePlan {
    let name = argument.trim();
    if snapshot.routes.is_empty() {
        return RoutePlan::Notice(
            "no routes configured — add one under .mjolnr/routes/".to_owned(),
        );
    }
    match by {
        RouteBy::Name => {
            if name.is_empty() {
                return RoutePlan::Notice(format!(
                    "usage: /route <name> — {}",
                    route_names(snapshot)
                ));
            }
            if snapshot.routes.iter().any(|choice| choice.name == name) {
                RoutePlan::Attach {
                    route: Some(name.to_owned()),
                    role: None,
                }
            } else {
                RoutePlan::Notice(format!(
                    "no route named `{name}` — {}",
                    route_names(snapshot)
                ))
            }
        }
        RouteBy::Role => {
            if name.is_empty() {
                return RoutePlan::Notice(format!(
                    "usage: /role <role> — {}",
                    role_names(snapshot)
                ));
            }
            if snapshot
                .routes
                .iter()
                .any(|choice| choice.roles.iter().any(|role| role == name))
            {
                RoutePlan::Attach {
                    route: None,
                    role: Some(name.to_owned()),
                }
            } else {
                RoutePlan::Notice(format!(
                    "no route is tagged role `{name}` — {}",
                    role_names(snapshot)
                ))
            }
        }
    }
}

/// What `/persona <argument>` should do, decided against the offered personas
/// so an unknown name is a stated notice rather than a silent no-op — the same
/// posture [`RoutePlan`] takes (§1.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PersonaPlan {
    /// Overlay the named persona (`Some`) or clear the override (`None`).
    Select(Option<String>),
    Notice(String),
}

/// Decide what `/persona <argument>` should do.
pub(crate) fn plan_persona_command(argument: &str, snapshot: &RuntimeSnapshot) -> PersonaPlan {
    let name = argument.trim();
    if name.is_empty() {
        let active = snapshot
            .active_persona
            .as_deref()
            .map_or_else(|| "none active".to_owned(), |name| format!("on `{name}`"));
        return PersonaPlan::Notice(format!(
            "usage: /persona <name|off> — {active}; {}",
            persona_names(snapshot)
        ));
    }
    // A deliberate clear, spelled the way the hint advertises.
    if name.eq_ignore_ascii_case("off") || name.eq_ignore_ascii_case("none") {
        return PersonaPlan::Select(None);
    }
    if snapshot.personas.is_empty() {
        return PersonaPlan::Notice(
            "no personas configured — add one under .mjolnr/personas/".to_owned(),
        );
    }
    if snapshot.personas.iter().any(|choice| choice.name == name) {
        PersonaPlan::Select(Some(name.to_owned()))
    } else {
        PersonaPlan::Notice(format!(
            "no persona named `{name}` — {}",
            persona_names(snapshot)
        ))
    }
}

fn persona_names(snapshot: &RuntimeSnapshot) -> String {
    if snapshot.personas.is_empty() {
        return "none configured".to_owned();
    }
    let names: Vec<&str> = snapshot
        .personas
        .iter()
        .map(|choice| choice.name.as_str())
        .collect();
    format!("available: {}", join_bounded(&names))
}

fn route_names(snapshot: &RuntimeSnapshot) -> String {
    let names: Vec<&str> = snapshot
        .routes
        .iter()
        .map(|choice| choice.name.as_str())
        .collect();
    format!("available: {}", join_bounded(&names))
}

fn role_names(snapshot: &RuntimeSnapshot) -> String {
    let roles: std::collections::BTreeSet<&str> = snapshot
        .routes
        .iter()
        .flat_map(|choice| choice.roles.iter().map(String::as_str))
        .collect();
    if roles.is_empty() {
        return "no routes carry role tags".to_owned();
    }
    let roles: Vec<&str> = roles.into_iter().collect();
    format!("available: {}", join_bounded(&roles))
}

fn join_bounded(names: &[&str]) -> String {
    let shown = names
        .iter()
        .take(MAX_LISTED_NAMES)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > MAX_LISTED_NAMES {
        format!("{shown}, … (+{})", names.len() - MAX_LISTED_NAMES)
    } else {
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_in_registry_has_a_non_empty_description() {
        for cmd in COMMANDS {
            assert!(
                !cmd.summary.trim().is_empty(),
                "Command {} has an empty description",
                cmd.name
            );
        }
    }

    #[test]
    fn a_bare_slash_lists_everything() {
        assert_eq!(matching("/").len(), COMMANDS.len());
    }

    fn snapshot_with_routes() -> RuntimeSnapshot {
        use crate::core::model::{ModelId, ProviderId};
        use crate::core::runtime::RouteChoice;
        RuntimeSnapshot {
            routes: std::sync::Arc::new(vec![
                RouteChoice {
                    name: "main".to_owned(),
                    roles: vec!["default".to_owned()],
                    provider: ProviderId::new("openai"),
                    model: ModelId::new("gpt-5.4"),
                    persona: None,
                },
                RouteChoice {
                    name: "cheap".to_owned(),
                    roles: vec!["smol".to_owned(), "child".to_owned()],
                    provider: ProviderId::new("openai"),
                    model: ModelId::new("gpt-5-mini"),
                    persona: None,
                },
            ]),
            ..RuntimeSnapshot::default()
        }
    }

    fn snapshot_with_personas() -> RuntimeSnapshot {
        use crate::core::context::{PersonaSummary, SkillScope};
        RuntimeSnapshot {
            personas: std::sync::Arc::new(vec![
                PersonaSummary {
                    name: "architect".to_owned(),
                    description: Some("weighs trade-offs".to_owned()),
                    scope: SkillScope::Project,
                },
                PersonaSummary {
                    name: "terse".to_owned(),
                    description: None,
                    scope: SkillScope::User,
                },
            ]),
            ..RuntimeSnapshot::default()
        }
    }

    #[test]
    fn a_known_persona_is_selected() {
        let plan = plan_persona_command("architect", &snapshot_with_personas());
        assert_eq!(plan, PersonaPlan::Select(Some("architect".to_owned())));
    }

    #[test]
    fn off_clears_the_persona_override_even_with_none_configured() {
        assert_eq!(
            plan_persona_command("off", &RuntimeSnapshot::default()),
            PersonaPlan::Select(None)
        );
    }

    #[test]
    fn an_unknown_persona_is_a_notice_not_a_selection() {
        let plan = plan_persona_command("ghost", &snapshot_with_personas());
        assert!(matches!(plan, PersonaPlan::Notice(text) if text.contains("ghost")));
    }

    #[test]
    fn a_bare_persona_command_explains_itself() {
        let plan = plan_persona_command("  ", &snapshot_with_personas());
        assert!(matches!(plan, PersonaPlan::Notice(text) if text.contains("usage")));
    }

    #[test]
    fn a_known_route_name_attaches_by_name() {
        let plan = plan_route_command(RouteBy::Name, "cheap", &snapshot_with_routes());
        assert_eq!(
            plan,
            RoutePlan::Attach {
                route: Some("cheap".to_owned()),
                role: None,
            }
        );
    }

    #[test]
    fn a_known_role_attaches_by_role() {
        let plan = plan_route_command(RouteBy::Role, "smol", &snapshot_with_routes());
        assert_eq!(
            plan,
            RoutePlan::Attach {
                route: None,
                role: Some("smol".to_owned()),
            }
        );
    }

    #[test]
    fn an_unknown_route_is_a_notice_not_a_silent_no_op() {
        let plan = plan_route_command(RouteBy::Name, "ghost", &snapshot_with_routes());
        match plan {
            RoutePlan::Notice(text) => assert!(text.contains("no route named `ghost`")),
            RoutePlan::Attach { .. } => panic!("an unknown route must never dispatch AttachRoute"),
        }
    }

    #[test]
    fn an_unknown_role_is_a_notice_not_a_silent_no_op() {
        let plan = plan_route_command(RouteBy::Role, "ghost", &snapshot_with_routes());
        match plan {
            RoutePlan::Notice(text) => assert!(text.contains("role `ghost`")),
            RoutePlan::Attach { .. } => panic!("an unknown role must never dispatch AttachRoute"),
        }
    }

    #[test]
    fn no_routing_config_explains_itself() {
        let plan = plan_route_command(RouteBy::Name, "main", &RuntimeSnapshot::default());
        match plan {
            RoutePlan::Notice(text) => assert!(text.contains("no routes configured")),
            RoutePlan::Attach { .. } => panic!("no config can attach nothing"),
        }
    }

    #[test]
    fn a_bare_route_command_lists_the_choices() {
        let plan = plan_route_command(RouteBy::Name, "  ", &snapshot_with_routes());
        match plan {
            RoutePlan::Notice(text) => {
                assert!(text.contains("main") && text.contains("cheap"));
            }
            RoutePlan::Attach { .. } => panic!("an empty argument must not attach"),
        }
    }

    #[test]
    fn matching_is_a_prefix_over_names_and_aliases() {
        let by_name = matching("/mod");
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name.first().expect("one match").name, "/model");

        // `/login` is an alias of `/auth`; it must resolve to the same row
        // rather than appearing as a command of its own.
        let by_alias = matching("/log");
        assert_eq!(by_alias.len(), 1);
        assert_eq!(by_alias.first().expect("one match").name, "/auth");
    }

    #[test]
    fn the_menu_closes_once_arguments_begin() {
        assert!(menu_applies("/mod"));
        assert!(menu_applies("/"));
        assert!(!menu_applies("/model openai gpt-5.4"));
        assert!(!menu_applies("write a test"));
    }

    #[test]
    fn unknown_input_matches_nothing() {
        assert!(matching("/nope").is_empty());
    }

    fn view_with_templates(names: &[&str]) -> ViewState {
        let snapshot = crate::core::runtime::RuntimeSnapshot {
            prompts: std::sync::Arc::new(
                names
                    .iter()
                    .map(|name| crate::core::context::PromptSummary {
                        name: (*name).to_owned(),
                        description: format!("template {name}"),
                        argument_hint: None,
                        scope: crate::core::context::SkillScope::Project,
                    })
                    .collect(),
            ),
            ..crate::core::runtime::RuntimeSnapshot::default()
        };
        ViewState {
            snapshot,
            ..ViewState::default()
        }
    }

    #[test]
    fn a_template_appears_in_the_menu_after_the_builtins() {
        let view = view_with_templates(&["review"]);
        let entries = menu_entries("/", &view);
        assert_eq!(entries.len(), COMMANDS.len() + 1);
        let last = entries.last().expect("a row");
        assert_eq!(last.name, "/review");
        assert!(last.template);
    }

    #[test]
    fn a_template_may_not_shadow_a_builtin_command() {
        // The collision rule: built-ins own their names. A template called
        // `model` must not appear as a second `/model` row, because the
        // dispatcher would never reach it.
        let view = view_with_templates(&["model"]);
        let entries = menu_entries("/model", &view);
        assert!(
            entries.iter().all(|entry| !entry.template),
            "a shadowed template is not offered"
        );
        assert!(is_builtin("/model"));
        assert!(is_builtin("/models"), "aliases are owned too");
        assert!(!is_builtin("/review"));
    }

    #[test]
    fn templates_filter_by_prefix_like_builtins() {
        let view = view_with_templates(&["review", "release"]);
        let entries = menu_entries("/rev", &view);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries.first().expect("one row").name, "/review");
    }
}
