//! Syntax extraction for source languages other than Rust.
//!
//! One reason to change: the tree-sitter node shapes mjolnr understands for
//! foreign-language imports and definitions. This module deliberately stops
//! at syntax. It does not run a compiler, load a language server, or infer a
//! dependency from a name that was not present in the source tree.

use tree_sitter::{Node, Parser};

use super::{SourceLanguage, SymbolKind};

#[derive(Debug, Default)]
pub(super) struct Extracted {
    /// Import strings exactly as written, without surrounding quote marks.
    pub uses: Vec<String>,
    /// Named definitions, as (name, kind, 1-based line).
    pub symbols: Vec<(String, SymbolKind, usize)>,
    /// Import-shaped syntax that could not be reduced to one path.
    pub unparsed_uses: usize,
}

enum ImportMatch {
    NotImport,
    Parsed(String),
    Unparsed,
}

pub(super) fn extract(language: SourceLanguage, source: &str) -> Extracted {
    let mut parser = Parser::new();
    let Some(parser_language) = language_for(language) else {
        return Extracted {
            unparsed_uses: 1,
            ..Extracted::default()
        };
    };
    if parser.set_language(&parser_language).is_err() {
        return Extracted {
            unparsed_uses: 1,
            ..Extracted::default()
        };
    }
    let Some(tree) = parser.parse(source, None) else {
        return Extracted {
            unparsed_uses: 1,
            ..Extracted::default()
        };
    };

    let mut extracted = Extracted::default();
    visit(tree.root_node(), language, source, &mut extracted);
    extracted.uses.sort();
    extracted.symbols.sort_by_key(|(_, _, line)| *line);
    extracted
}

fn language_for(language: SourceLanguage) -> Option<tree_sitter::Language> {
    match language {
        SourceLanguage::JavaScript => Some(tree_sitter_javascript::LANGUAGE.into()),
        SourceLanguage::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        SourceLanguage::TypeScriptReact => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        SourceLanguage::Python => Some(tree_sitter_python::LANGUAGE.into()),
        SourceLanguage::Go => Some(tree_sitter_go::LANGUAGE.into()),
        SourceLanguage::Rust => None,
    }
}

fn visit(node: Node<'_>, language: SourceLanguage, source: &str, out: &mut Extracted) {
    match import_from_node(node, language, source) {
        ImportMatch::Parsed(path) => out.uses.push(path),
        ImportMatch::Unparsed => out.unparsed_uses = out.unparsed_uses.saturating_add(1),
        ImportMatch::NotImport => {}
    }
    if let Some((name, kind)) = symbol_from_node(node, language, source) {
        out.symbols
            .push((name, kind, node.start_position().row.saturating_add(1)));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit(child, language, source, out);
    }
}

fn import_from_node(node: Node<'_>, language: SourceLanguage, source: &str) -> ImportMatch {
    match language {
        SourceLanguage::JavaScript
        | SourceLanguage::TypeScript
        | SourceLanguage::TypeScriptReact => {
            if node.kind() == "import_statement" {
                return node
                    .child_by_field_name("source")
                    .and_then(|child| string_literal(child, source))
                    .map_or(ImportMatch::Unparsed, ImportMatch::Parsed);
            }
            if node.kind() == "export_statement" && node.child_by_field_name("source").is_some() {
                return node
                    .child_by_field_name("source")
                    .and_then(|child| string_literal(child, source))
                    .map_or(ImportMatch::Unparsed, ImportMatch::Parsed);
            }
            if node.kind() == "call_expression" && is_require_call(node, source) {
                return node
                    .child_by_field_name("arguments")
                    .and_then(|arguments| arguments.named_child(0))
                    .and_then(|child| string_literal(child, source))
                    .map_or(ImportMatch::Unparsed, ImportMatch::Parsed);
            }
        }
        SourceLanguage::Python => {
            if node.kind() == "import_statement" {
                return python_import(node, source)
                    .map_or(ImportMatch::Unparsed, ImportMatch::Parsed);
            }
            if node.kind() == "import_from_statement" {
                return node
                    .child_by_field_name("module_name")
                    .and_then(|child| node_text(child, source))
                    .or_else(|| python_from_module(node, source))
                    .map_or(ImportMatch::Unparsed, ImportMatch::Parsed);
            }
        }
        SourceLanguage::Go => {
            if node.kind() == "import_spec" {
                return first_string_literal(node, source)
                    .map_or(ImportMatch::Unparsed, ImportMatch::Parsed);
            }
        }
        SourceLanguage::Rust => {}
    }
    ImportMatch::NotImport
}

fn symbol_from_node(
    node: Node<'_>,
    language: SourceLanguage,
    source: &str,
) -> Option<(String, SymbolKind)> {
    let kind = match (language, node.kind()) {
        (
            SourceLanguage::JavaScript
            | SourceLanguage::TypeScript
            | SourceLanguage::TypeScriptReact,
            "function_declaration" | "method_definition",
        )
        | (SourceLanguage::Python, "function_definition")
        | (SourceLanguage::Go, "function_declaration" | "method_declaration") => {
            SymbolKind::Function
        }
        (
            SourceLanguage::JavaScript
            | SourceLanguage::TypeScript
            | SourceLanguage::TypeScriptReact,
            "class_declaration",
        )
        | (SourceLanguage::Python, "class_definition") => SymbolKind::Struct,
        (
            SourceLanguage::JavaScript
            | SourceLanguage::TypeScript
            | SourceLanguage::TypeScriptReact,
            "interface_declaration",
        ) => SymbolKind::Trait,
        (
            SourceLanguage::JavaScript
            | SourceLanguage::TypeScript
            | SourceLanguage::TypeScriptReact,
            "type_alias_declaration",
        )
        | (SourceLanguage::Go, "type_spec") => SymbolKind::TypeAlias,
        _ => return None,
    };
    let name = node.child_by_field_name("name")?;
    Some((node_text(name, source)?, kind))
}

fn is_require_call(node: Node<'_>, source: &str) -> bool {
    node.child_by_field_name("function")
        .and_then(|function| node_text(function, source))
        .is_some_and(|name| name == "require")
}

fn python_import(node: Node<'_>, source: &str) -> Option<String> {
    let text = node_text(node, source)?;
    let imports = text.strip_prefix("import")?.trim();
    let first = imports.split(',').next()?.trim();
    let path = first.split_whitespace().next()?.trim();
    (!path.is_empty()).then(|| path.to_owned())
}

fn python_from_module(node: Node<'_>, source: &str) -> Option<String> {
    let text = node_text(node, source)?;
    let rest = text.strip_prefix("from")?.trim();
    let module = rest.split_whitespace().next()?.trim();
    (!module.is_empty()).then(|| module.to_owned())
}

fn first_string_literal(node: Node<'_>, source: &str) -> Option<String> {
    if is_string_node(node) {
        return string_literal(node, source);
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find_map(|child| first_string_literal(child, source))
}

fn is_string_node(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "string" | "string_fragment" | "interpreted_string_literal" | "raw_string_literal"
    )
}

fn string_literal(node: Node<'_>, source: &str) -> Option<String> {
    let text_storage = node_text(node, source)?;
    let text = text_storage.trim();
    let without_prefix = text
        .strip_prefix("r\"")
        .or_else(|| text.strip_prefix("r'"))
        .unwrap_or(text);
    let value = without_prefix
        .strip_prefix('"')
        .or_else(|| without_prefix.strip_prefix('\''))?;
    let quote = without_prefix.chars().next()?;
    let value = value.strip_suffix(quote)?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn node_text(node: Node<'_>, source: &str) -> Option<String> {
    node.utf8_text(source.as_bytes()).ok().map(str::to_owned)
}
