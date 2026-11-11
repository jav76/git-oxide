use std::path::Path;
use std::sync::LazyLock;

use tree_sitter::Language;
use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter};

use crate::models::{CodeTokenSpan, RgbaColor};

/// Maximum blob size in bytes (1 MB) eligible for full-blob AST syntax highlighting.
pub const MAX_HIGHLIGHT_BLOB_BYTES: usize = 1_048_576;

/// Maximum characters on a single line before AST token spans are collapsed into one plain span.
pub const MAX_HIGHLIGHT_LINE_CHARS: usize = 500;

/// Maximum characters displayed for a single line to prevent minified files from stalling layout.
pub const MAX_DISPLAY_LINE_CHARS: usize = 2_000;

/// Default foreground color for plain source text (`#D8DEE9`).
pub const COLOR_DEFAULT: RgbaColor = RgbaColor::rgb(216, 222, 233);
/// Keyword color (`#C678DD`).
pub const COLOR_KEYWORD: RgbaColor = RgbaColor::rgb(198, 120, 221);
/// Type and struct identifier color (`#E5C07B`).
pub const COLOR_TYPE: RgbaColor = RgbaColor::rgb(229, 192, 123);
/// Function and method identifier color (`#61AFEF`).
pub const COLOR_FUNCTION: RgbaColor = RgbaColor::rgb(97, 175, 239);
/// String and character literal color (`#98C379`).
pub const COLOR_STRING: RgbaColor = RgbaColor::rgb(152, 195, 121);
/// Numeric and boolean constant color (`#D19A66`).
pub const COLOR_NUMBER: RgbaColor = RgbaColor::rgb(209, 154, 102);
/// Comment color (`#7F848E`).
pub const COLOR_COMMENT: RgbaColor = RgbaColor::rgb(127, 132, 142);
/// Constant and enum member color (`#56B6C2`).
pub const COLOR_CONSTANT: RgbaColor = RgbaColor::rgb(86, 182, 194);
/// Field and property identifier color (`#E06C75`).
pub const COLOR_PROPERTY: RgbaColor = RgbaColor::rgb(224, 108, 117);
/// Operator color (`#56B6C2`).
pub const COLOR_OPERATOR: RgbaColor = RgbaColor::rgb(86, 182, 194);
/// Punctuation and bracket color (`#ABB2BF`).
pub const COLOR_PUNCTUATION: RgbaColor = RgbaColor::rgb(171, 178, 191);
/// Attribute and macro decorator color (`#D19A66`).
pub const COLOR_ATTRIBUTE: RgbaColor = RgbaColor::rgb(209, 154, 102);
/// HTML/XML tag color (`#E06C75`).
pub const COLOR_TAG: RgbaColor = RgbaColor::rgb(224, 108, 117);

const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

const C_SHARP_HIGHLIGHT_QUERY: &str = r#"
(comment) @comment
(modifier) @keyword
(this_expression) @variable.builtin
(base_expression) @variable.builtin
(boolean_literal) @constant.builtin
(null_literal) @constant.builtin
(integer_literal) @number
(real_literal) @number
(string_literal) @string
(verbatim_string_literal) @string
(interpolated_string_expression) @string
(character_literal) @string
(predefined_type) @type.builtin
(class_declaration name: (identifier) @type)
(struct_declaration name: (identifier) @type)
(interface_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(record_declaration name: (identifier) @type)
(method_declaration name: (identifier) @function.method)
(local_function_statement name: (identifier) @function)
(invocation_expression function: (identifier) @function)
(invocation_expression function: (member_access_expression name: (identifier) @function.method))
(property_declaration name: (identifier) @property)
(attribute name: (identifier) @attribute)
[
  "using"
  "namespace"
  "class"
  "struct"
  "interface"
  "enum"
  "record"
  "public"
  "private"
  "protected"
  "internal"
  "static"
  "readonly"
  "const"
  "async"
  "await"
  "return"
  "if"
  "else"
  "switch"
  "case"
  "for"
  "foreach"
  "in"
  "while"
  "do"
  "break"
  "continue"
  "new"
  "try"
  "catch"
  "finally"
  "throw"
  "var"
  "is"
  "as"
] @keyword
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SupportedGrammar {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    C,
    Cpp,
    CSharp,
    Json,
    Toml,
    Html,
    Css,
    Bash,
    Markdown,
}

struct GrammarRegistry {
    rust: HighlightConfiguration,
    python: HighlightConfiguration,
    javascript: HighlightConfiguration,
    typescript: HighlightConfiguration,
    tsx: HighlightConfiguration,
    go: HighlightConfiguration,
    c: HighlightConfiguration,
    cpp: HighlightConfiguration,
    c_sharp: HighlightConfiguration,
    json: HighlightConfiguration,
    toml: HighlightConfiguration,
    html: HighlightConfiguration,
    css: HighlightConfiguration,
    bash: HighlightConfiguration,
    markdown: HighlightConfiguration,
}

static REGISTRY: LazyLock<GrammarRegistry> = LazyLock::new(|| GrammarRegistry {
    rust: build_config(Language::new(tree_sitter_rust::LANGUAGE), "rust", tree_sitter_rust::HIGHLIGHTS_QUERY),
    python: build_config(Language::new(tree_sitter_python::LANGUAGE), "python", tree_sitter_python::HIGHLIGHTS_QUERY),
    javascript: build_config(
        Language::new(tree_sitter_javascript::LANGUAGE),
        "javascript",
        tree_sitter_javascript::HIGHLIGHT_QUERY,
    ),
    typescript: build_config(
        Language::new(tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
        "typescript",
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
    ),
    tsx: build_config(
        Language::new(tree_sitter_typescript::LANGUAGE_TSX),
        "tsx",
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
    ),
    go: build_config(Language::new(tree_sitter_go::LANGUAGE), "go", tree_sitter_go::HIGHLIGHTS_QUERY),
    c: build_config(Language::new(tree_sitter_c::LANGUAGE), "c", tree_sitter_c::HIGHLIGHT_QUERY),
    cpp: build_config(Language::new(tree_sitter_cpp::LANGUAGE), "cpp", tree_sitter_cpp::HIGHLIGHT_QUERY),
    c_sharp: build_config(Language::new(tree_sitter_c_sharp::LANGUAGE), "c_sharp", C_SHARP_HIGHLIGHT_QUERY),
    json: build_config(Language::new(tree_sitter_json::LANGUAGE), "json", tree_sitter_json::HIGHLIGHTS_QUERY),
    toml: build_config(Language::new(tree_sitter_toml_ng::LANGUAGE), "toml", tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
    html: build_config(Language::new(tree_sitter_html::LANGUAGE), "html", tree_sitter_html::HIGHLIGHTS_QUERY),
    css: build_config(Language::new(tree_sitter_css::LANGUAGE), "css", tree_sitter_css::HIGHLIGHTS_QUERY),
    bash: build_config(Language::new(tree_sitter_bash::LANGUAGE), "bash", tree_sitter_bash::HIGHLIGHT_QUERY),
    markdown: build_config(Language::new(tree_sitter_md::LANGUAGE), "markdown", tree_sitter_md::HIGHLIGHT_QUERY_BLOCK),
});

fn build_config(language: Language, name: &str, highlights_query: &str) -> HighlightConfiguration {
    let mut config: HighlightConfiguration = HighlightConfiguration::new(language, name, highlights_query, "", "")
        .unwrap_or_else(|_| {
            HighlightConfiguration::new(Language::new(tree_sitter_json::LANGUAGE), name, "", "", "")
                .expect("empty query configuration is always valid")
        });
    config.configure(HIGHLIGHT_NAMES);
    config
}

fn color_for_highlight(highlight: Highlight) -> RgbaColor {
    let name: &str = HIGHLIGHT_NAMES.get(highlight.0).copied().unwrap_or("");
    match name {
        "attribute" | "function.macro" => COLOR_ATTRIBUTE,
        "comment" => COLOR_COMMENT,
        "constant" | "constant.builtin" => COLOR_CONSTANT,
        "constructor" | "type" | "type.builtin" | "module" => COLOR_TYPE,
        "function" | "function.builtin" | "function.method" => COLOR_FUNCTION,
        "keyword" | "label" => COLOR_KEYWORD,
        "number" => COLOR_NUMBER,
        "operator" => COLOR_OPERATOR,
        "property" | "variable.builtin" => COLOR_PROPERTY,
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter" | "punctuation.special" => COLOR_PUNCTUATION,
        "string" | "string.special" | "escape" => COLOR_STRING,
        "tag" => COLOR_TAG,
        _ => COLOR_DEFAULT,
    }
}

fn detect_grammar(file_path: &str) -> Option<SupportedGrammar> {
    let path: &Path = Path::new(file_path);
    let extension: String = path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase)?;

    match extension.as_str() {
        "rs" => Some(SupportedGrammar::Rust),
        "py" | "pyi" => Some(SupportedGrammar::Python),
        "js" | "mjs" | "cjs" | "jsx" => Some(SupportedGrammar::JavaScript),
        "ts" | "mts" | "cts" => Some(SupportedGrammar::TypeScript),
        "tsx" => Some(SupportedGrammar::Tsx),
        "go" => Some(SupportedGrammar::Go),
        "c" | "h" => Some(SupportedGrammar::C),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Some(SupportedGrammar::Cpp),
        "cs" => Some(SupportedGrammar::CSharp),
        "json" => Some(SupportedGrammar::Json),
        "toml" => Some(SupportedGrammar::Toml),
        "html" | "htm" | "xml" | "svg" => Some(SupportedGrammar::Html),
        "css" | "scss" => Some(SupportedGrammar::Css),
        "sh" | "bash" | "zsh" => Some(SupportedGrammar::Bash),
        "md" | "markdown" => Some(SupportedGrammar::Markdown),
        _ => None,
    }
}

fn config_for_grammar(grammar: SupportedGrammar) -> &'static HighlightConfiguration {
    let registry: &GrammarRegistry = &REGISTRY;
    match grammar {
        SupportedGrammar::Rust => &registry.rust,
        SupportedGrammar::Python => &registry.python,
        SupportedGrammar::JavaScript => &registry.javascript,
        SupportedGrammar::TypeScript => &registry.typescript,
        SupportedGrammar::Tsx => &registry.tsx,
        SupportedGrammar::Go => &registry.go,
        SupportedGrammar::C => &registry.c,
        SupportedGrammar::Cpp => &registry.cpp,
        SupportedGrammar::CSharp => &registry.c_sharp,
        SupportedGrammar::Json => &registry.json,
        SupportedGrammar::Toml => &registry.toml,
        SupportedGrammar::Html => &registry.html,
        SupportedGrammar::Css => &registry.css,
        SupportedGrammar::Bash => &registry.bash,
        SupportedGrammar::Markdown => &registry.markdown,
    }
}

/// Highlights a full file blob using `tree-sitter-highlight` and returns per-line `Coalesced Token Spans`.
///
/// Enforces the 1 MB blob limit and 500-character line limit defined in `ADR-0002`.
pub fn highlight_blob_lines(file_path: &str, content: &str) -> Vec<Vec<CodeTokenSpan>> {
    if content.is_empty() {
        return Vec::new();
    }

    if content.len() > MAX_HIGHLIGHT_BLOB_BYTES {
        log::debug!(
            "skipping AST syntax highlighting for '{file_path}' ({} bytes > {MAX_HIGHLIGHT_BLOB_BYTES})",
            content.len()
        );
        return fallback_plain_lines(content);
    }

    let Some(grammar) = detect_grammar(file_path) else {
        return fallback_plain_lines(content);
    };

    let config: &HighlightConfiguration = config_for_grammar(grammar);
    let mut highlighter: Highlighter = Highlighter::new();
    let Ok(events) = highlighter.highlight(config, content.as_bytes(), None, |_| None) else {
        return fallback_plain_lines(content);
    };

    let mut lines: Vec<Vec<CodeTokenSpan>> = Vec::new();
    let mut current_line: Vec<CodeTokenSpan> = Vec::new();
    let mut current_line_chars: usize = 0;
    let mut style_stack: Vec<Highlight> = Vec::new();

    for event_result in events {
        let Ok(event) = event_result else {
            return fallback_plain_lines(content);
        };
        match event {
            HighlightEvent::Source { start, end } => {
                let Some(slice) = content.get(start..end) else {
                    continue;
                };
                let color: RgbaColor = style_stack.last().copied().map_or(COLOR_DEFAULT, color_for_highlight);

                let mut segments = slice.split('\n').peekable();
                while let Some(segment) = segments.next() {
                    let clean_segment: &str = segment.strip_suffix('\r').unwrap_or(segment);
                    if !clean_segment.is_empty() {
                        append_coalesced_span(&mut current_line, &mut current_line_chars, clean_segment, color);
                    }
                    if segments.peek().is_some() {
                        finalize_line(&mut current_line, current_line_chars);
                        lines.push(std::mem::take(&mut current_line));
                        current_line_chars = 0;
                    }
                }
            }
            HighlightEvent::HighlightStart(highlight) => {
                style_stack.push(highlight);
            }
            HighlightEvent::HighlightEnd => {
                let _ = style_stack.pop();
            }
        }
    }

    if !current_line.is_empty() || !content.ends_with('\n') {
        finalize_line(&mut current_line, current_line_chars);
        lines.push(current_line);
    }

    lines
}

fn append_coalesced_span(
    line_spans: &mut Vec<CodeTokenSpan>,
    line_char_count: &mut usize,
    segment: &str,
    color: RgbaColor,
) {
    if *line_char_count >= MAX_DISPLAY_LINE_CHARS {
        return;
    }

    let remaining_budget: usize = MAX_DISPLAY_LINE_CHARS.saturating_sub(*line_char_count);
    let mut clipped: String = String::new();
    let mut added_chars: usize = 0;
    for ch in segment.chars().take(remaining_budget) {
        if ch == '\t' {
            clipped.push_str("    ");
            added_chars += 4;
        } else {
            clipped.push(ch);
            added_chars += 1;
        }
    }
    *line_char_count += added_chars;

    if let Some(last_span) = line_spans.last_mut()
        && last_span.color == color
    {
        last_span.text.push_str(&clipped);
        return;
    }

    line_spans.push(CodeTokenSpan { text: clipped, color });
}

fn finalize_line(line_spans: &mut Vec<CodeTokenSpan>, line_char_count: usize) {
    if line_spans.is_empty() {
        line_spans.push(CodeTokenSpan { text: String::new(), color: COLOR_DEFAULT });
        return;
    }

    // ADR-0002 guardrail: collapse lines exceeding 500 chars into a single plain span.
    if line_char_count > MAX_HIGHLIGHT_LINE_CHARS && line_spans.len() > 1 {
        let mut combined: String = String::with_capacity(line_char_count);
        for span in line_spans.iter() {
            combined.push_str(&span.text);
        }
        line_spans.clear();
        line_spans.push(CodeTokenSpan { text: combined, color: COLOR_DEFAULT });
    }
}

/// Produces plain default-colored single-span lines when AST highlighting is bypassed.
pub fn fallback_plain_lines(content: &str) -> Vec<Vec<CodeTokenSpan>> {
    content
        .lines()
        .map(|line| {
            let mut formatted: String = String::new();
            for ch in line.chars().take(MAX_DISPLAY_LINE_CHARS) {
                if ch == '\t' {
                    formatted.push_str("    ");
                } else {
                    formatted.push(ch);
                }
            }
            vec![CodeTokenSpan { text: formatted, color: COLOR_DEFAULT }]
        })
        .collect()
}
