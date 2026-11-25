//! Small native lexer for bounded source previews. Call from a background worker;
//! retain the lexer at page boundaries instead of rescanning the whole file.
use std::{ops::Range, path::Path};

pub const KEYWORD: u32 = 0xbdabf3;
pub const STRING: u32 = 0xb5cba5;
pub const COMMENT: u32 = 0x747d8b;
pub const NUMBER: u32 = 0xe6b77e;
pub const TYPE: u32 = 0x7dd3fc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    Javascript,
    Typescript,
    Json,
    Jsonc,
    C,
    Cpp,
    Swift,
    Go,
    Java,
    Kotlin,
    Ruby,
    Shell,
    Toml,
    Yaml,
    Sql,
    Css,
    Html,
    Xml,
}
impl Language {
    pub fn for_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        if matches!(name, ".bashrc" | ".zshrc" | ".bash_profile" | ".profile") {
            return Some(Self::Shell);
        }
        Self::for_name(path.extension()?.to_str()?)
    }
    pub fn for_name(name: &str) -> Option<Self> {
        // A language hint is at most a small extension/fence label; no lower-case
        // allocation is needed for repeated preview or diff lines.
        [
            (Self::Rust, &["rs", "rust"] as &[&str]),
            (Self::Python, &["py", "pyi", "python"]),
            (Self::Javascript, &["js", "jsx", "mjs", "cjs", "javascript"]),
            (Self::Typescript, &["ts", "tsx", "typescript"]),
            (Self::Json, &["json", "jsonl"]),
            (Self::Jsonc, &["jsonc"]),
            (Self::C, &["c", "h"]),
            (Self::Cpp, &["cpp", "hpp", "cc", "cxx", "hxx", "c++"]),
            (Self::Swift, &["swift"]),
            (Self::Go, &["go", "golang"]),
            (Self::Java, &["java"]),
            (Self::Kotlin, &["kotlin", "kt", "kts"]),
            (Self::Ruby, &["rb", "ruby"]),
            (Self::Shell, &["sh", "bash", "zsh", "shell"]),
            (Self::Toml, &["toml"]),
            (Self::Yaml, &["yaml", "yml"]),
            (Self::Sql, &["sql"]),
            (Self::Css, &["css", "scss"]),
            (Self::Html, &["html", "htm"]),
            (Self::Xml, &["xml"]),
        ]
        .into_iter()
        .find_map(|(language, aliases)| {
            aliases
                .iter()
                .any(|alias| name.eq_ignore_ascii_case(alias))
                .then_some(language)
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Javascript => "javascript",
            Self::Typescript => "typescript",
            Self::Json => "json",
            Self::Jsonc => "jsonc",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Swift => "swift",
            Self::Go => "go",
            Self::Java => "java",
            Self::Kotlin => "kotlin",
            Self::Ruby => "ruby",
            Self::Shell => "shell",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
            Self::Sql => "sql",
            Self::Css => "css",
            Self::Html => "html",
            Self::Xml => "xml",
        }
    }
    fn hash_comments(self) -> bool {
        matches!(
            self,
            Self::Python | Self::Ruby | Self::Shell | Self::Toml | Self::Yaml
        )
    }
    fn c_comments(self) -> bool {
        matches!(
            self,
            Self::Rust
                | Self::Javascript
                | Self::Typescript
                | Self::C
                | Self::Cpp
                | Self::Swift
                | Self::Go
                | Self::Java
                | Self::Kotlin
                | Self::Css
                | Self::Jsonc
                | Self::Sql
        )
    }
    fn markup(self) -> bool {
        matches!(self, Self::Html | Self::Xml)
    }
    fn keyword(self, word: &str) -> bool {
        let keywords: &[&str] = match self {
            Self::Rust => &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
                "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
                "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
                "super", "trait", "true", "type", "unsafe", "use", "where", "while",
            ],
            Self::Python => &[
                "and", "as", "assert", "async", "await", "break", "class", "continue", "def",
                "del", "elif", "else", "except", "finally", "for", "from", "global", "if",
                "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return",
                "try", "while", "with", "yield",
            ],
            Self::Javascript | Self::Typescript => &[
                "abstract",
                "as",
                "async",
                "await",
                "break",
                "case",
                "catch",
                "class",
                "const",
                "continue",
                "debugger",
                "declare",
                "default",
                "delete",
                "do",
                "else",
                "enum",
                "export",
                "extends",
                "finally",
                "for",
                "from",
                "function",
                "get",
                "if",
                "implements",
                "import",
                "in",
                "instanceof",
                "interface",
                "keyof",
                "let",
                "namespace",
                "new",
                "of",
                "private",
                "protected",
                "public",
                "readonly",
                "return",
                "set",
                "static",
                "super",
                "switch",
                "this",
                "throw",
                "try",
                "type",
                "typeof",
                "var",
                "void",
                "while",
                "yield",
            ],
            Self::C => &[
                "auto", "break", "case", "char", "const", "continue", "default", "do", "double",
                "else", "enum", "extern", "float", "for", "if", "inline", "int", "long",
                "register", "restrict", "return", "short", "signed", "sizeof", "static", "struct",
                "switch", "typedef", "union", "unsigned", "void", "volatile", "while",
            ],
            Self::Cpp => &[
                "auto",
                "bool",
                "break",
                "case",
                "char",
                "class",
                "const",
                "constexpr",
                "continue",
                "default",
                "delete",
                "do",
                "double",
                "else",
                "enum",
                "extern",
                "float",
                "for",
                "if",
                "inline",
                "int",
                "long",
                "namespace",
                "new",
                "nullptr",
                "private",
                "protected",
                "public",
                "register",
                "return",
                "short",
                "signed",
                "sizeof",
                "static",
                "struct",
                "switch",
                "template",
                "typedef",
                "typename",
                "union",
                "unsigned",
                "using",
                "virtual",
                "void",
                "volatile",
                "while",
            ],
            Self::Swift => &[
                "actor",
                "any",
                "as",
                "async",
                "await",
                "break",
                "case",
                "class",
                "continue",
                "defer",
                "do",
                "else",
                "enum",
                "extension",
                "fallthrough",
                "fileprivate",
                "for",
                "func",
                "guard",
                "if",
                "import",
                "in",
                "inout",
                "internal",
                "is",
                "let",
                "mutating",
                "nonisolated",
                "open",
                "override",
                "private",
                "protocol",
                "public",
                "repeat",
                "rethrows",
                "return",
                "self",
                "some",
                "static",
                "struct",
                "super",
                "switch",
                "throw",
                "throws",
                "try",
                "typealias",
                "var",
                "where",
                "while",
            ],
            Self::Go => &[
                "break",
                "case",
                "chan",
                "const",
                "continue",
                "default",
                "defer",
                "else",
                "fallthrough",
                "for",
                "func",
                "go",
                "goto",
                "if",
                "import",
                "interface",
                "map",
                "package",
                "range",
                "return",
                "select",
                "struct",
                "switch",
                "type",
                "var",
            ],
            Self::Java => &[
                "abstract",
                "boolean",
                "break",
                "byte",
                "case",
                "catch",
                "char",
                "class",
                "const",
                "continue",
                "default",
                "do",
                "double",
                "else",
                "enum",
                "extends",
                "final",
                "finally",
                "float",
                "for",
                "if",
                "implements",
                "import",
                "instanceof",
                "int",
                "interface",
                "long",
                "native",
                "new",
                "package",
                "private",
                "protected",
                "public",
                "return",
                "short",
                "static",
                "super",
                "switch",
                "synchronized",
                "this",
                "throw",
                "throws",
                "transient",
                "try",
                "void",
                "volatile",
                "while",
            ],
            Self::Kotlin => &[
                "as",
                "break",
                "class",
                "continue",
                "do",
                "else",
                "false",
                "for",
                "fun",
                "if",
                "in",
                "interface",
                "is",
                "null",
                "object",
                "package",
                "return",
                "super",
                "this",
                "throw",
                "true",
                "try",
                "typealias",
                "typeof",
                "val",
                "var",
                "when",
                "while",
                "by",
                "catch",
                "constructor",
                "delegate",
                "dynamic",
                "field",
                "file",
                "finally",
                "get",
                "import",
                "init",
                "param",
                "property",
                "receiver",
                "set",
                "setparam",
                "where",
                "abstract",
                "actual",
                "annotation",
                "companion",
                "const",
                "crossinline",
                "data",
                "enum",
                "expect",
                "external",
                "final",
                "infix",
                "inline",
                "inner",
                "internal",
                "lateinit",
                "noinline",
                "open",
                "operator",
                "out",
                "override",
                "private",
                "protected",
                "public",
                "reified",
                "sealed",
                "suspend",
                "tailrec",
                "vararg",
            ],
            Self::Ruby => &[
                "alias", "and", "begin", "break", "case", "class", "def", "defined", "do", "else",
                "elsif", "end", "ensure", "for", "if", "in", "module", "next", "not", "or", "redo",
                "rescue", "retry", "return", "self", "super", "then", "undef", "unless", "until",
                "when", "while", "yield",
            ],
            Self::Shell => &[
                "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function",
                "if", "in", "local", "readonly", "return", "select", "then", "until", "while",
            ],
            Self::Sql => &[
                "select",
                "from",
                "where",
                "join",
                "inner",
                "left",
                "right",
                "outer",
                "on",
                "group",
                "by",
                "order",
                "having",
                "limit",
                "offset",
                "insert",
                "into",
                "values",
                "update",
                "set",
                "delete",
                "create",
                "table",
                "index",
                "drop",
                "alter",
                "and",
                "or",
                "not",
                "as",
                "distinct",
                "union",
                "all",
                "exists",
                "case",
                "when",
                "then",
                "else",
                "end",
                "primary",
                "key",
                "references",
                "with",
                "returning",
            ],
            _ => &[],
        };
        keywords.iter().any(|keyword| {
            if self == Self::Sql {
                word.eq_ignore_ascii_case(keyword)
            } else {
                word == *keyword
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
enum State {
    #[default]
    Normal,
    String {
        quote: u8,
        triple: bool,
        raw_hashes: Option<u8>,
    },
    BlockComment(u16),
    LineComment,
    MarkupComment,
}
#[derive(Clone, Debug)]
pub struct Lexer {
    language: Language,
    state: State,
    in_tag: bool,
}
impl Lexer {
    pub fn new(language: Language) -> Self {
        Self {
            language,
            state: State::Normal,
            in_tag: false,
        }
    }
    pub fn language(&self) -> Language {
        self.language
    }
    /// Advance an omitted real line terminator without allocating a throwaway
    /// highlight span. Do not call this between chunks of the same long line.
    pub fn end_line(&mut self) {
        if matches!(self.state, State::LineComment) {
            self.state = State::Normal;
        }
    }
    /// Byte ranges always fall on UTF-8 boundaries; whitespace and punctuation
    /// retain the surrounding text color. No scan of prior chunks is performed.
    pub fn highlight(&mut self, text: &str) -> Vec<(Range<usize>, u32)> {
        let bytes = text.as_bytes();
        let mut spans: Vec<(Range<usize>, u32)> = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let start = at;
            if matches!(self.state, State::Normal) {
                if self.language.markup() && bytes[at..].starts_with(b"<!--") {
                    at += 4;
                    self.state = State::MarkupComment;
                } else if self.language.c_comments() && bytes[at..].starts_with(b"/*") {
                    at += 2;
                    self.state = State::BlockComment(1);
                } else if (self.language.c_comments()
                    && !matches!(self.language, Language::Css | Language::Sql)
                    && bytes[at..].starts_with(b"//"))
                    || (self.language == Language::Sql && bytes[at..].starts_with(b"--"))
                    || (self.language.hash_comments()
                        && bytes[at] == b'#'
                        && (!matches!(self.language, Language::Shell | Language::Yaml)
                            || at == 0
                            || bytes[at - 1].is_ascii_whitespace()))
                {
                    self.state = State::LineComment;
                } else if let Some((prefix, hashes)) = self.rust_raw(&bytes[at..]) {
                    at += prefix;
                    self.state = State::String {
                        quote: b'"',
                        triple: false,
                        raw_hashes: Some(hashes),
                    };
                } else if self.is_quote(text, at) {
                    let quote = bytes[at];
                    let triple = matches!(self.language, Language::Python | Language::Toml)
                        && bytes
                            .get(at..at + 3)
                            .is_some_and(|slice| slice == [quote, quote, quote]);
                    at += if triple { 3 } else { 1 };
                    self.state = State::String {
                        quote,
                        triple,
                        raw_hashes: None,
                    };
                }
            }
            let color = match self.state {
                State::String {
                    quote,
                    triple,
                    raw_hashes,
                } => {
                    while at < bytes.len() {
                        if raw_hashes.is_none() && bytes[at] == b'\\' {
                            at += 1;
                            if at < bytes.len() {
                                at += text[at..].chars().next().unwrap().len_utf8();
                            }
                        } else if bytes[at] == quote
                            && if let Some(hashes) = raw_hashes {
                                bytes
                                    .get(at + 1..at + 1 + hashes as usize)
                                    .is_some_and(|slice| slice.iter().all(|b| *b == b'#'))
                            } else {
                                !triple
                                    || bytes
                                        .get(at..at + 3)
                                        .is_some_and(|slice| slice == [quote, quote, quote])
                            }
                        {
                            at += raw_hashes
                                .map_or(if triple { 3 } else { 1 }, |hashes| 1 + hashes as usize);
                            self.state = State::Normal;
                            break;
                        } else {
                            at += text[at..].chars().next().unwrap().len_utf8();
                        }
                    }
                    Some(STRING)
                }
                State::LineComment => {
                    while at < bytes.len() && bytes[at] != b'\n' {
                        at += text[at..].chars().next().unwrap().len_utf8();
                    }
                    if at < bytes.len() {
                        at += 1;
                        self.state = State::Normal;
                    }
                    Some(COMMENT)
                }
                State::BlockComment(mut depth) => {
                    while at < bytes.len() {
                        if bytes[at..].starts_with(b"*/") {
                            at += 2;
                            depth -= 1;
                            if depth == 0 {
                                self.state = State::Normal;
                                break;
                            }
                        } else if matches!(self.language, Language::Rust | Language::Swift)
                            && bytes[at..].starts_with(b"/*")
                        {
                            depth = depth.saturating_add(1);
                            at += 2;
                        } else {
                            at += text[at..].chars().next().unwrap().len_utf8();
                        }
                    }
                    if depth > 0 {
                        self.state = State::BlockComment(depth);
                    }
                    Some(COMMENT)
                }
                State::MarkupComment => {
                    while at < bytes.len() {
                        if bytes[at..].starts_with(b"-->") {
                            at += 3;
                            self.state = State::Normal;
                            break;
                        }
                        at += text[at..].chars().next().unwrap().len_utf8();
                    }
                    Some(COMMENT)
                }
                State::Normal => {
                    let ch = text[at..].chars().next().unwrap();
                    if self.language.markup() && ch == '<' {
                        self.in_tag = true;
                        at += 1;
                        Some(KEYWORD)
                    } else if self.language.markup() && ch == '>' {
                        self.in_tag = false;
                        at += 1;
                        Some(KEYWORD)
                    } else if ch.is_ascii_digit() && !self.language.markup() {
                        at += 1;
                        while at < bytes.len()
                            && (bytes[at].is_ascii_alphanumeric()
                                || matches!(bytes[at], b'.' | b'_'))
                        {
                            at += 1;
                        }
                        Some(NUMBER)
                    } else if ch.is_alphabetic() || ch == '_' {
                        at += ch.len_utf8();
                        while at < bytes.len() {
                            let ch = text[at..].chars().next().unwrap();
                            if !ch.is_alphanumeric() && ch != '_' {
                                break;
                            }
                            at += ch.len_utf8();
                        }
                        let token = &text[start..at];
                        if self.language.keyword(token) {
                            Some(KEYWORD)
                        } else if matches!(
                            token,
                            "true"
                                | "false"
                                | "null"
                                | "nil"
                                | "None"
                                | "True"
                                | "False"
                                | "undefined"
                        ) {
                            Some(NUMBER)
                        } else if self.language.markup() && self.in_tag
                            || matches!(
                                self.language,
                                Language::Rust
                                    | Language::Swift
                                    | Language::Go
                                    | Language::Java
                                    | Language::Kotlin
                                    | Language::Cpp
                                    | Language::Typescript
                            ) && token.chars().next().is_some_and(|ch| ch.is_uppercase())
                        {
                            Some(TYPE)
                        } else {
                            None
                        }
                    } else {
                        at += ch.len_utf8();
                        None
                    }
                }
            };
            if let Some(color) = color {
                if let Some((range, previous)) = spans.last_mut()
                    && *previous == color
                    && range.end == start
                {
                    range.end = at;
                } else {
                    spans.push((start..at, color));
                }
            }
        }
        spans
    }
    fn rust_raw(&self, bytes: &[u8]) -> Option<(usize, u8)> {
        if self.language != Language::Rust {
            return None;
        }
        let mut at = if bytes.starts_with(b"br") {
            2
        } else if bytes.starts_with(b"r") {
            1
        } else {
            return None;
        };
        let initial = at;
        while bytes.get(at) == Some(&b'#') && at - initial < 32 {
            at += 1;
        }
        (bytes.get(at) == Some(&b'"')).then_some((at + 1, (at - initial) as u8))
    }
    fn is_quote(&self, text: &str, at: usize) -> bool {
        let byte = text.as_bytes()[at];
        if self.language.markup() && !self.in_tag {
            return false;
        }
        if byte == b'"' {
            return true;
        }
        if byte == b'`' {
            return matches!(
                self.language,
                Language::Javascript | Language::Typescript | Language::Go | Language::Shell
            );
        }
        if byte != b'\'' || matches!(self.language, Language::Json | Language::Jsonc) {
            return false;
        }
        if self.language != Language::Rust {
            return true;
        }
        // Rust lifetimes are not string literals. Character literals must close
        // immediately after one scalar or their escape sequence.
        let suffix = &text[at + 1..];
        if let Some(escape) = suffix.strip_prefix('\\') {
            return escape.contains('\'');
        }
        suffix
            .chars()
            .next()
            .is_some_and(|ch| suffix.as_bytes().get(ch.len_utf8()) == Some(&b'\''))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn colored<'a>(text: &'a str, spans: &[(Range<usize>, u32)], color: u32) -> Vec<&'a str> {
        spans
            .iter()
            .filter(|(_, c)| *c == color)
            .map(|(range, _)| &text[range.clone()])
            .collect()
    }
    #[test]
    fn source_extensions_are_specific_and_plain_text_has_no_lexer() {
        assert_eq!(
            Language::for_path(Path::new("main.RS")),
            Some(Language::Rust)
        );
        assert_eq!(
            Language::for_path(Path::new("component.tsx")),
            Some(Language::Typescript)
        );
        assert_eq!(
            Language::for_path(Path::new(".zshrc")),
            Some(Language::Shell)
        );
        assert_eq!(Language::for_path(Path::new("notes.txt")), None);
        assert_eq!(Language::for_path(Path::new("README.md")), None);
        assert_eq!(Language::for_name("unknown"), None);
    }
    #[test]
    fn rust_unicode_raw_strings_and_lifetimes_have_valid_ranges() {
        let text = "pub fn café<'a>(s: &'a str) { let x = r##\"🦀 // literal\"##; let c = '🦀'; }";
        let spans = Lexer::new(Language::Rust).highlight(text);
        assert_eq!(
            colored(text, &spans, KEYWORD),
            vec!["pub", "fn", "let", "let"]
        );
        assert_eq!(
            colored(text, &spans, STRING),
            vec!["r##\"🦀 // literal\"##", "'🦀'"]
        );
        assert!(colored(text, &spans, COMMENT).is_empty());
        for (range, _) in spans {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        }
    }
    #[test]
    fn cloned_page_state_keeps_nested_comments_and_multiline_strings() {
        let mut lexer = Lexer::new(Language::Rust);
        lexer.highlight("/* outer /* nested\n");
        let mut next = lexer.clone();
        let text = "*/ still comment */ let x = 4;";
        let spans = next.highlight(text);
        assert_eq!(colored(text, &spans, COMMENT), vec!["*/ still comment */"]);
        assert_eq!(colored(text, &spans, KEYWORD), vec!["let"]);
        let mut python = Lexer::new(Language::Python);
        python.highlight("x = \"\"\"🦀\n");
        let text = "continued\"\"\"\nreturn 1";
        let spans = python.highlight(text);
        assert_eq!(colored(text, &spans, STRING), vec!["continued\"\"\""]);
        assert_eq!(colored(text, &spans, KEYWORD), vec!["return"]);
    }
    #[test]
    fn only_language_keywords_are_colored_and_comment_syntax_is_specific() {
        let rust = "const class = 1; // def\n";
        let spans = Lexer::new(Language::Rust).highlight(rust);
        assert_eq!(colored(rust, &spans, KEYWORD), vec!["const"]);
        let py = "x = a // 2 # comment\n";
        let spans = Lexer::new(Language::Python).highlight(py);
        assert_eq!(colored(py, &spans, COMMENT), vec!["# comment\n"]);
        let sql = "SELECT name FROM users -- comment\n";
        let spans = Lexer::new(Language::Sql).highlight(sql);
        assert_eq!(colored(sql, &spans, KEYWORD), vec!["SELECT", "FROM"]);
        let html = "Hello <div title=\"🦀\">world</div>";
        let spans = Lexer::new(Language::Html).highlight(html);
        assert!(
            !spans.iter().any(
                |(range, _)| &html[range.clone()] == "Hello" || &html[range.clone()] == "world"
            )
        );
    }
}
