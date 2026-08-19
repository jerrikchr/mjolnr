//! In-app controls and slash-command reference.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::tui::layout::centered;
use crate::tui::theme;

pub(super) fn render(frame: &mut Frame, area: Rect) {
    let rows = [
        ("CTRL-C", "interrupt work / clear input; twice quits"),
        ("CTRL-D", "quit from an empty idle directive"),
        ("ESC", "interrupt active work / dismiss this panel"),
        ("ENTER", "send directive"),
        ("CTRL-J / SHIFT-ENTER", "insert newline"),
        ("CMD-V/C · CTRL-V/Y", "paste / copy draft or latest output"),
        ("SHIFT-TAB", "cycle read-only / ask / workspace-write"),
        ("CTRL-O", "show or hide tool-result detail"),
        ("CTRL-P", "jump palette; CTRL-A goes to attention queue"),
        ("CTRL-PGUP / PGDN", "previous / next workspace surface"),
        ("PAGE UP / PAGE DOWN", "move through transcript history"),
        ("END", "jump to newest transcript entry"),
        ("CTRL-L", "redraw terminal"),
        ("F1", "toggle this control surface"),
        ("/", "list commands; TAB completes a single match"),
        ("y / n / a", "approval only: once / deny / exact command"),
        ("c / e", "recovery only: abandon and continue / end session"),
    ];
    let mut lines = vec![
        Line::from(Span::styled(
            "CONTEXT-AWARE COMMAND CHANNEL",
            theme::muted(),
        )),
        Line::from(""),
    ];
    lines.extend(rows.into_iter().map(|(key, meaning)| {
        Line::from(vec![
            Span::styled(format!("{key:<23}"), theme::title()),
            Span::styled(meaning, theme::text()),
        ])
    }));

    // The commands themselves come from the one registry rather than a second
    // hand-maintained list here, which would drift the moment a command is added.
    // Two columns of name-and-shape, not name-and-summary: the registry grows
    // over time, and a one-per-row list clips its own tail at a short terminal
    // height. The summary and live state for each command already live one
    // keystroke away in the `/` menu, so this panel shows what exists and how
    // to spell it, and lets that menu describe it.
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "COMMANDS  ·  type / for descriptions",
        theme::muted(),
    )));
    let names: Vec<String> = crate::tui::commands::COMMANDS
        .iter()
        .map(|command| match command.hint {
            Some(hint) => format!("{} {hint}", command.name),
            None => command.name.to_owned(),
        })
        .collect();
    lines.extend(names.chunks(2).map(|pair| {
        let left = pair.first().map(String::as_str).unwrap_or_default();
        let right = pair.get(1).map(String::as_str).unwrap_or_default();
        Line::from(vec![
            Span::styled(format!("{left:<36}"), theme::title()),
            Span::styled(right.to_owned(), theme::title()),
        ])
    }));

    // Sized to content: the row list grows with the command registry, and a
    // fixed height would silently clip whatever was added last.
    let modal = centered(
        area,
        area.width.saturating_sub(4).min(78),
        u16::try_from(lines.len() + 2)
            .unwrap_or(u16::MAX)
            .min(area.height),
    );
    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::modal())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme::chrome())
                    .title(Span::styled(" KEYMAP ", theme::title())),
            ),
        modal,
    );
}
