//! Rust source extraction: text in, declarations out.
//!
//! One reason to change: the syntax of the declarations mjolnr reads.
//!
//! This is a bounded line scanner, not a parser, and the distinction is load
//! bearing. It reads `use`, `mod`, and item-definition lines and understands
//! nothing else — no macro expansion, no `cfg` evaluation, no type resolution.
//! Everything it cannot resolve is *counted*, never guessed at, so a caller can
//! tell "no importers" from "the scanner gave up" (`AGENTS.md` §1.3).
//!
//! `tree-sitter` would parse this properly and is deliberately not taken: its
//! grammars are C, and this codebase has twice paid to avoid a C system
//! dependency (`syntect` on `regex-fancy` rather than oniguruma, `ratatui-image`
//! without `chafa`). A retrieval convenience does not get to reverse a
//! security-surface decision that syntax highlighting was denied.

/// What kind of item a definition site introduces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    TypeAlias,
    Const,
    Static,
    Macro,
    Module,
}

impl SymbolKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::TypeAlias => "type",
            Self::Const => "const",
            Self::Static => "static",
            Self::Macro => "macro",
            Self::Module => "mod",
        }
    }
}

/// Everything one file declares, in source order.
#[derive(Debug, Default)]
pub(super) struct Extracted {
    /// Fully expanded `use` paths, brace groups flattened.
    pub uses: Vec<String>,
    /// `mod name;` declarations — the file's children.
    pub child_modules: Vec<String>,
    /// Named items, as (name, kind, 1-based line).
    pub symbols: Vec<(String, SymbolKind, usize)>,
    /// `use` statements the scanner could not expand into paths.
    pub unparsed_uses: usize,
}

/// Modifiers that may precede an item keyword, longest first so that
/// `pub(crate)` is stripped before `pub`.
const MODIFIERS: &[&str] = &[
    "pub(crate)",
    "pub(super)",
    "pub(self)",
    "pub",
    "default",
    "async",
    "unsafe",
    "extern",
    "const",
];

/// Keyword to kind. `const` is both a modifier and an item keyword, so it is
/// matched here after modifier stripping has already run once.
const KEYWORDS: &[(&str, SymbolKind)] = &[
    ("fn ", SymbolKind::Function),
    ("struct ", SymbolKind::Struct),
    ("enum ", SymbolKind::Enum),
    ("trait ", SymbolKind::Trait),
    ("type ", SymbolKind::TypeAlias),
    ("const ", SymbolKind::Const),
    ("static ", SymbolKind::Static),
    ("macro_rules! ", SymbolKind::Macro),
    ("mod ", SymbolKind::Module),
];

pub(super) fn extract(source: &str) -> Extracted {
    let mut out = Extracted::default();
    let mut pending: Option<String> = None;
    let mut in_block_comment = false;

    for (offset, raw) in source.lines().enumerate() {
        let line = strip_comments(raw, &mut in_block_comment);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(mut accumulated) = pending.take() {
            accumulated.push(' ');
            accumulated.push_str(trimmed);
            finish_or_hold(accumulated, &mut pending, &mut out);
            continue;
        }

        if let Some(rest) = use_statement(trimmed) {
            finish_or_hold(rest.to_owned(), &mut pending, &mut out);
            continue;
        }

        collect_declaration(trimmed, offset.saturating_add(1), &mut out);
    }

    // A `use` still open at end of file never terminated; it is unresolvable
    // rather than absent, and the count is the honest way to say so.
    if pending.is_some() {
        out.unparsed_uses = out.unparsed_uses.saturating_add(1);
    }
    out
}

/// Hold an incomplete `use` statement, or expand a complete one.
fn finish_or_hold(statement: String, pending: &mut Option<String>, out: &mut Extracted) {
    let Some(body) = statement.split(';').next() else {
        out.unparsed_uses = out.unparsed_uses.saturating_add(1);
        return;
    };
    if !statement.contains(';') {
        *pending = Some(statement);
        return;
    }
    let mut expanded = Vec::new();
    if expand(body.trim(), &mut expanded) {
        out.uses.extend(expanded);
    } else {
        out.unparsed_uses = out.unparsed_uses.saturating_add(1);
    }
}

/// The body of a `use` statement, with visibility stripped.
fn use_statement(trimmed: &str) -> Option<&str> {
    for prefix in ["use ", "pub use ", "pub(crate) use ", "pub(super) use "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

/// Expand one `use` body into concrete paths, flattening brace groups.
///
/// Returns `false` when the shape is not understood, so the caller can count it
/// as unresolved instead of emitting a path that was never written.
fn expand(body: &str, out: &mut Vec<String>) -> bool {
    let body = body.trim();
    if body.is_empty() {
        return false;
    }
    let Some(open) = body.find('{') else {
        out.push(normalize_leaf(body));
        return true;
    };
    let Some(close) = body.rfind('}') else {
        return false;
    };
    let (prefix, remainder) = (body.get(..open), body.get(open.saturating_add(1)..close));
    let (Some(prefix), Some(inner)) = (prefix, remainder) else {
        return false;
    };
    let prefix = prefix.trim().trim_end_matches("::");

    for part in split_top_level(inner) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let joined = if part == "self" {
            prefix.to_owned()
        } else {
            format!("{prefix}::{part}")
        };
        if !expand(&joined, out) {
            return false;
        }
    }
    true
}

/// Split a brace group on commas that are not inside a nested group.
fn split_top_level(inner: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0_usize;
    let mut current = String::new();
    for character in inner.chars() {
        match character {
            '{' => {
                depth = depth.saturating_add(1);
                current.push(character);
            }
            '}' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    parts.push(current);
    parts
}

/// Drop an `as` alias and a trailing glob; neither changes which module the
/// import reaches.
fn normalize_leaf(path: &str) -> String {
    let path = path.split(" as ").next().unwrap_or(path).trim();
    path.trim_end_matches("::*").trim().to_owned()
}

fn collect_declaration(trimmed: &str, line: usize, out: &mut Extracted) {
    if let Some(name) = child_module(trimmed) {
        out.child_modules.push(name);
        return;
    }
    let stripped = strip_modifiers(trimmed);
    for (keyword, kind) in KEYWORDS {
        if let Some(rest) = stripped.strip_prefix(keyword)
            && let Some(name) = identifier(rest)
        {
            out.symbols.push((name, *kind, line));
            return;
        }
    }
}

/// `mod name;` — a declaration that points at another file, as opposed to
/// `mod name {`, which is an inline module and merely a symbol.
fn child_module(trimmed: &str) -> Option<String> {
    let stripped = strip_modifiers(trimmed);
    let rest = stripped.strip_prefix("mod ")?;
    let name = rest.trim().strip_suffix(';')?;
    let name = name.trim();
    name.chars()
        .all(|character| character.is_alphanumeric() || character == '_')
        .then(|| name.to_owned())
        .filter(|name| !name.is_empty())
}

fn strip_modifiers(line: &str) -> &str {
    let mut current = line.trim();
    // Bounded rather than `loop`: a declaration cannot carry more modifiers
    // than there are modifiers, and an unbounded loop over model-adjacent text
    // is how a scanner becomes a hang.
    for _ in 0..MODIFIERS.len() {
        let before = current;
        for modifier in MODIFIERS {
            if let Some(rest) = current.strip_prefix(modifier)
                && rest.starts_with(|character: char| character.is_whitespace())
                && strippable(modifier, rest.trim_start())
            {
                current = rest.trim_start();
                break;
            }
        }
        if before == current {
            break;
        }
    }
    current
}

/// `const` is the one word that is both a modifier and an item keyword.
///
/// Stripping it unconditionally turns `const N: u8 = 1;` into `N: u8 = 1;`,
/// which matches nothing, and the constant vanishes from the index — a silently
/// incomplete answer, which is the failure mode this module is built to avoid.
/// It is a modifier only in `const fn`.
fn strippable(modifier: &str, rest: &str) -> bool {
    if modifier != "const" {
        return true;
    }
    rest.starts_with("fn ") || rest.starts_with("unsafe ") || rest.starts_with("extern ")
}

/// The identifier a keyword introduces, cut at the first character that can
/// follow a name.
fn identifier(rest: &str) -> Option<String> {
    let name: String = rest
        .trim_start()
        .chars()
        .take_while(|character| character.is_alphanumeric() || *character == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Remove comments so they cannot contribute declarations.
///
/// Approximate by construction: it does not track string literals, so a `//`
/// inside a string truncates the line. That costs a declaration on a line no
/// Rust file writes, and the alternative is a lexer.
fn strip_comments(raw: &str, in_block_comment: &mut bool) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    loop {
        if *in_block_comment {
            let Some(end) = rest.find("*/") else {
                return out;
            };
            rest = rest.get(end.saturating_add(2)..).unwrap_or("");
            *in_block_comment = false;
            continue;
        }
        let line_comment = rest.find("//");
        let block_comment = rest.find("/*");
        // A line comment wins only when it starts first; otherwise the block
        // comment opens and the rest of the line is scanned inside it.
        let line_first = match (line_comment, block_comment) {
            (Some(line), Some(block)) => line < block,
            (Some(_), None) => true,
            (None, _) => false,
        };
        match (line_first, line_comment, block_comment) {
            (true, Some(line), _) => {
                out.push_str(rest.get(..line).unwrap_or(""));
                return out;
            }
            (_, _, Some(start)) => {
                out.push_str(rest.get(..start).unwrap_or(""));
                rest = rest.get(start.saturating_add(2)..).unwrap_or("");
                *in_block_comment = true;
            }
            _ => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_brace_groups_including_nested_ones() {
        let extracted = extract("use crate::a::{b, c::{d, e}};\n");
        assert_eq!(
            extracted.uses,
            vec!["crate::a::b", "crate::a::c::d", "crate::a::c::e"]
        );
        assert_eq!(extracted.unparsed_uses, 0);
    }

    #[test]
    fn self_in_a_group_resolves_to_the_prefix() {
        // Deliberately invented module names. Real ones (`crate::store::…`)
        // read as genuine imports to `tests/architecture.rs`, which scans source
        // text rather than the resolved module graph — fixture data must not
        // look like the thing the scanner is hunting for.
        let extracted = extract("use crate::alpha::{self, beta};\n");
        assert_eq!(extracted.uses, vec!["crate::alpha", "crate::alpha::beta"]);
    }

    #[test]
    fn aliases_and_globs_normalize_to_the_module_reached() {
        let extracted = extract("use crate::a::B as C;\nuse crate::d::*;\n");
        assert_eq!(extracted.uses, vec!["crate::a::B", "crate::d"]);
    }

    #[test]
    fn a_use_spanning_lines_is_one_statement() {
        let extracted = extract("use crate::a::{\n    b,\n    c,\n};\n");
        assert_eq!(extracted.uses, vec!["crate::a::b", "crate::a::c"]);
        assert_eq!(extracted.unparsed_uses, 0);
    }

    #[test]
    fn declarations_are_found_behind_visibility_and_modifiers() {
        let source = "pub(crate) async fn run() {}\npub struct Thing;\nenum E {}\n\
                      pub trait T {}\ntype Alias = u8;\nconst N: u8 = 1;\n\
                      static S: u8 = 1;\nmacro_rules! m {}\n";
        let kinds: Vec<_> = extract(source)
            .symbols
            .into_iter()
            .map(|(name, kind, _)| (name, kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("run".to_owned(), SymbolKind::Function),
                ("Thing".to_owned(), SymbolKind::Struct),
                ("E".to_owned(), SymbolKind::Enum),
                ("T".to_owned(), SymbolKind::Trait),
                ("Alias".to_owned(), SymbolKind::TypeAlias),
                ("N".to_owned(), SymbolKind::Const),
                ("S".to_owned(), SymbolKind::Static),
                ("m".to_owned(), SymbolKind::Macro),
            ]
        );
    }

    #[test]
    fn a_file_declaration_is_a_child_and_an_inline_module_is_a_symbol() {
        let extracted = extract("mod child;\npub mod inline {\n}\n");
        assert_eq!(extracted.child_modules, vec!["child"]);
        assert_eq!(
            extracted.symbols,
            vec![("inline".to_owned(), SymbolKind::Module, 2)]
        );
    }

    #[test]
    fn commented_out_declarations_do_not_count() {
        let source =
            "// fn ghost() {}\n/* struct Ghost; */\n/*\nfn also_ghost() {}\n*/\nfn real() {}\n";
        let names: Vec<_> = extract(source)
            .symbols
            .into_iter()
            .map(|(name, _, line)| (name, line))
            .collect();
        assert_eq!(names, vec![("real".to_owned(), 6)]);
    }

    #[test]
    fn const_is_an_item_keyword_and_only_a_modifier_before_fn() {
        let extracted =
            extract("pub const LIMIT: usize = 8;\npub const fn limit() -> usize { 8 }\n");
        assert_eq!(
            extracted.symbols,
            vec![
                ("LIMIT".to_owned(), SymbolKind::Const, 1),
                ("limit".to_owned(), SymbolKind::Function, 2),
            ]
        );
    }

    #[test]
    fn lines_are_one_based() {
        let extracted = extract("\n\nfn third_line() {}\n");
        assert_eq!(extracted.symbols.first().map(|entry| entry.2), Some(3));
    }
}
