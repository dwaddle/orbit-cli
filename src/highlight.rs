use colored::Colorize;
use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    Bash,
    Json,
    Toml,
    Yaml,
    Sql,
    Html,
    Css,
    C,
    Cpp,
    Markdown,
    Unknown,
}

impl Language {
    pub fn from_tag(tag: &str) -> Self {
        let clean = tag.trim().to_lowercase();
        match clean.as_str() {
            "rs" | "rust" => Language::Rust,
            "py" | "python" | "python3" => Language::Python,
            "js" | "javascript" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "ts" | "typescript" | "tsx" => Language::TypeScript,
            "go" | "golang" => Language::Go,
            "sh" | "bash" | "zsh" | "shell" => Language::Bash,
            "json" | "jsonc" => Language::Json,
            "toml" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            "sql" => Language::Sql,
            "html" | "htm" => Language::Html,
            "css" | "scss" | "sass" => Language::Css,
            "c" => Language::C,
            "cpp" | "c++" | "cxx" | "cc" => Language::Cpp,
            "md" | "markdown" => Language::Markdown,
            _ => Language::Unknown,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Python => "python",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Go => "go",
            Language::Bash => "bash",
            Language::Json => "json",
            Language::Toml => "toml",
            Language::Yaml => "yaml",
            Language::Sql => "sql",
            Language::Html => "html",
            Language::Css => "css",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Markdown => "markdown",
            Language::Unknown => "text",
        }
    }
}

pub struct StreamingHighlighter {
    pub enabled: bool,
    buffer: String,
    in_code_block: bool,
    current_lang: Language,
    code_line_count: usize,
}

impl StreamingHighlighter {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            buffer: String::new(),
            in_code_block: false,
            current_lang: Language::Unknown,
            code_line_count: 0,
        }
    }

    /// Feed a streaming token chunk from the LLM.
    pub fn feed(&mut self, chunk: &str) {
        if !self.enabled {
            print!("{}", chunk);
            let _ = io::stdout().flush();
            return;
        }

        self.buffer.push_str(chunk);

        while let Some(newline_pos) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=newline_pos).collect();
            let line_trimmed = line.trim_end_matches(['\r', '\n']);
            self.process_line(line_trimmed);
        }
    }

    /// Process a single complete line.
    fn process_line(&mut self, line: &str) {
        let trimmed_start = line.trim_start();
        if trimmed_start.starts_with("```") {
            if !self.in_code_block {
                // Entering code block
                let tag = trimmed_start.trim_start_matches('`').trim();
                self.current_lang = Language::from_tag(tag);
                self.in_code_block = true;
                self.code_line_count = 0;

                let lang_label = if tag.is_empty() {
                    "code"
                } else {
                    self.current_lang.display_name()
                };

                let header = format!("╭─── [{}] {}", lang_label, "─".repeat(50));
                println!("{}", header.cyan().bold());
            } else {
                // Exiting code block
                self.in_code_block = false;
                self.current_lang = Language::Unknown;
                self.code_line_count = 0;

                let footer = format!("╰{}", "─".repeat(60));
                println!("{}", footer.cyan().bold());
            }
            let _ = io::stdout().flush();
            return;
        }

        if self.in_code_block {
            self.code_line_count += 1;
            let highlighted = highlight_code_line(line, self.current_lang);
            let border = "│ ".dimmed();
            println!("{}{}", border, highlighted);
        } else {
            let highlighted = highlight_markdown_line(line);
            println!("{}", highlighted);
        }
        let _ = io::stdout().flush();
    }

    /// Flush any remaining partial text in the buffer when the stream ends.
    pub fn flush(&mut self) {
        if !self.enabled {
            let _ = io::stdout().flush();
            return;
        }

        if !self.buffer.is_empty() {
            let remaining = std::mem::take(&mut self.buffer);
            if self.in_code_block {
                let highlighted = highlight_code_line(&remaining, self.current_lang);
                let border = "│ ".dimmed();
                print!("{}{}", border, highlighted);
            } else {
                let highlighted = highlight_markdown_line(&remaining);
                print!("{}", highlighted);
            }
            let _ = io::stdout().flush();
        }

        if self.in_code_block {
            // Unclosed code block guard
            let footer = format!("╰{}", "─".repeat(60));
            println!("{}", footer.cyan().bold());
            self.in_code_block = false;
            let _ = io::stdout().flush();
        }
    }
}

/// Highlight a single line of code according to language syntax rules.
pub fn highlight_code_line(line: &str, lang: Language) -> String {
    match lang {
        Language::Rust => highlight_rust(line),
        Language::Python => highlight_python(line),
        Language::JavaScript | Language::TypeScript => highlight_javascript_typescript(line),
        Language::Go => highlight_go(line),
        Language::Bash => highlight_bash(line),
        Language::Json => highlight_json(line),
        Language::Toml | Language::Yaml => highlight_toml_yaml(line),
        Language::Sql => highlight_sql(line),
        Language::C | Language::Cpp => highlight_c_cpp(line),
        Language::Html | Language::Css | Language::Markdown | Language::Unknown => {
            highlight_generic(line)
        }
    }
}

/// Highlight a markdown line (headers, bullets, blockquotes, inline formatting).
pub fn highlight_markdown_line(line: &str) -> String {
    let trimmed = line.trim_start();

    // Headers
    if trimmed.starts_with("### ") {
        return line.bold().cyan().to_string();
    } else if trimmed.starts_with("## ") {
        return line.bold().bright_blue().to_string();
    } else if trimmed.starts_with("# ") {
        return line.bold().magenta().to_string();
    }

    // Blockquotes
    if trimmed.starts_with("> ") {
        let bar = "│ ".cyan().bold().to_string();
        let content = &line[line.find('>').unwrap() + 1..];
        return format!("{}{}", bar, content.dimmed().italic());
    }

    // Unordered lists
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        let bullet = "•".bright_green().bold().to_string();
        let prefix_len = line.len() - trimmed.len();
        let indent = &line[..prefix_len];
        let rest = &trimmed[2..];
        return format!("{}{} {}", indent, bullet, highlight_inline_spans(rest));
    }

    // Numbered lists
    if let Some(dot_pos) = trimmed.find(". ") {
        let prefix_slice = &trimmed[..dot_pos];
        if prefix_slice.chars().all(|c| c.is_ascii_digit()) && !prefix_slice.is_empty() {
            let num = prefix_slice.yellow().bold().to_string();
            let prefix_len = line.len() - trimmed.len();
            let indent = &line[..prefix_len];
            let rest = &trimmed[dot_pos + 2..];
            return format!("{}{} {}", indent, num, highlight_inline_spans(rest));
        }
    }

    highlight_inline_spans(line)
}

/// Format inline markdown spans: `code` -> yellow, **bold** -> bold, *italic* -> italic.
pub fn highlight_inline_spans(text: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '`' {
            let mut code = String::new();
            let mut closed = false;
            let mut j = i + 1;
            while j < len {
                if chars[j] == '`' {
                    closed = true;
                    j += 1;
                    break;
                }
                code.push(chars[j]);
                j += 1;
            }
            if closed {
                result.push_str(&format!("`{}`", code).yellow().bold().to_string());
                i = j;
            } else {
                result.push('`');
                i += 1;
            }
        } else if chars[i] == '*' && i + 1 < len && chars[i + 1] == '*' {
            let mut bold_text = String::new();
            let mut closed = false;
            let mut j = i + 2;
            while j + 1 < len {
                if chars[j] == '*' && chars[j + 1] == '*' {
                    closed = true;
                    j += 2;
                    break;
                }
                bold_text.push(chars[j]);
                j += 1;
            }
            if closed {
                result.push_str(&bold_text.bold().to_string());
                i = j;
            } else {
                result.push_str("**");
                i += 2;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

// -----------------------------------------------------------------------------
// Language Highlighters
// -----------------------------------------------------------------------------

fn highlight_rust(line: &str) -> String {
    let keywords = [
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while",
    ];

    let types = [
        "String", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
        "i128", "isize", "f32", "f64", "bool", "char", "Option", "Result", "Some", "None", "Ok",
        "Err", "Vec", "HashMap", "HashSet", "BTreeMap", "BTreeSet", "Box", "Rc", "Arc", "Mutex",
        "RwLock", "Path", "PathBuf",
    ];

    tokenize_and_color(line, &keywords, &types, "//", false)
}

fn highlight_python(line: &str) -> String {
    let keywords = [
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
        "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True",
        "try", "while", "with", "yield",
    ];

    let types = [
        "int",
        "str",
        "float",
        "bool",
        "list",
        "dict",
        "set",
        "tuple",
        "bytes",
        "object",
        "self",
        "cls",
        "Exception",
        "print",
        "len",
        "range",
        "enumerate",
        "zip",
        "isinstance",
        "type",
    ];

    tokenize_and_color(line, &keywords, &types, "#", true)
}

fn highlight_javascript_typescript(line: &str) -> String {
    let keywords = [
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "export",
        "extends",
        "finally",
        "for",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "new",
        "return",
        "super",
        "switch",
        "this",
        "throw",
        "try",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        "yield",
        "enum",
        "implements",
        "package",
        "protected",
        "static",
        "interface",
        "type",
        "from",
        "as",
        "true",
        "false",
        "null",
        "undefined",
    ];

    let types = [
        "string", "number", "boolean", "any", "void", "never", "unknown", "object", "symbol",
        "Promise", "Array", "Record", "Map", "Set", "Function", "Error", "console",
    ];

    tokenize_and_color(line, &keywords, &types, "//", false)
}

fn highlight_go(line: &str) -> String {
    let keywords = [
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
        "true",
        "false",
        "nil",
        "iota",
    ];

    let types = [
        "bool",
        "string",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "uint",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "uintptr",
        "byte",
        "rune",
        "float32",
        "float64",
        "complex64",
        "complex128",
        "error",
        "make",
        "new",
        "append",
        "len",
        "cap",
        "copy",
        "panic",
        "recover",
    ];

    tokenize_and_color(line, &keywords, &types, "//", false)
}

fn highlight_bash(line: &str) -> String {
    let keywords = [
        "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until",
        "do", "done", "in", "function", "time", "return", "exit", "echo", "export", "local",
        "source", "set", "read", "cd", "alias",
    ];

    let builtins = [
        "cat", "grep", "sed", "awk", "cut", "tr", "find", "xargs", "curl", "chmod",
    ];

    tokenize_and_color(line, &keywords, &builtins, "#", true)
}

fn highlight_sql(line: &str) -> String {
    let keywords = [
        "SELECT",
        "FROM",
        "WHERE",
        "JOIN",
        "LEFT",
        "RIGHT",
        "INNER",
        "OUTER",
        "CROSS",
        "ON",
        "GROUP",
        "BY",
        "ORDER",
        "HAVING",
        "LIMIT",
        "OFFSET",
        "INSERT",
        "INTO",
        "VALUES",
        "UPDATE",
        "SET",
        "DELETE",
        "CREATE",
        "TABLE",
        "VIEW",
        "INDEX",
        "ALTER",
        "DROP",
        "TRUNCATE",
        "PRIMARY",
        "KEY",
        "FOREIGN",
        "REFERENCES",
        "AND",
        "OR",
        "NOT",
        "IN",
        "IS",
        "NULL",
        "LIKE",
        "BETWEEN",
        "EXISTS",
        "CASE",
        "WHEN",
        "THEN",
        "ELSE",
        "END",
        "AS",
        "DISTINCT",
        "UNION",
        "ALL",
        "ASC",
        "DESC",
        "COUNT",
        "SUM",
        "AVG",
        "MIN",
        "MAX",
    ];

    let types = [
        "INTEGER",
        "INT",
        "BIGINT",
        "SMALLINT",
        "VARCHAR",
        "CHAR",
        "TEXT",
        "BOOLEAN",
        "DATE",
        "TIMESTAMP",
        "FLOAT",
        "DOUBLE",
        "DECIMAL",
        "NUMERIC",
        "BLOB",
    ];

    tokenize_and_color(line, &keywords, &types, "--", false)
}

fn highlight_c_cpp(line: &str) -> String {
    let keywords = [
        "auto",
        "break",
        "case",
        "char",
        "const",
        "continue",
        "default",
        "do",
        "double",
        "else",
        "enum",
        "extern",
        "float",
        "for",
        "goto",
        "if",
        "int",
        "long",
        "register",
        "return",
        "short",
        "signed",
        "sizeof",
        "static",
        "struct",
        "switch",
        "typedef",
        "union",
        "unsigned",
        "void",
        "volatile",
        "while",
        "class",
        "public",
        "private",
        "protected",
        "namespace",
        "using",
        "template",
        "typename",
        "new",
        "delete",
        "this",
        "nullptr",
        "true",
        "false",
    ];

    let types = [
        "size_t",
        "int8_t",
        "int16_t",
        "int32_t",
        "int64_t",
        "uint8_t",
        "uint16_t",
        "uint32_t",
        "uint64_t",
        "std",
        "string",
        "vector",
        "map",
        "unordered_map",
        "shared_ptr",
        "unique_ptr",
    ];

    tokenize_and_color(line, &keywords, &types, "//", false)
}

fn highlight_json(line: &str) -> String {
    let mut result = String::new();
    let mut in_string = false;
    let mut is_key = true;
    let mut string_buf = String::new();

    for ch in line.chars() {
        if ch == '"' {
            if in_string {
                string_buf.push('"');
                if is_key {
                    result.push_str(&string_buf.cyan().bold().to_string());
                } else {
                    result.push_str(&string_buf.green().to_string());
                }
                string_buf.clear();
                in_string = false;
            } else {
                in_string = true;
                string_buf.push('"');
            }
        } else if in_string {
            string_buf.push(ch);
        } else {
            if ch == ':' {
                is_key = false;
                result.push_str(&ch.to_string().white().to_string());
            } else if ch == ',' || ch == '{' || ch == '}' || ch == '[' || ch == ']' {
                if ch == ',' || ch == '{' || ch == '[' {
                    is_key = true;
                }
                result.push_str(&ch.to_string().white().to_string());
            } else if ch.is_ascii_digit() || ch == '.' || ch == '-' {
                result.push_str(&ch.to_string().yellow().to_string());
            } else if ch.is_whitespace() {
                result.push(ch);
            } else {
                result.push_str(&ch.to_string().magenta().to_string());
            }
        }
    }

    if !string_buf.is_empty() {
        result.push_str(&string_buf.green().to_string());
    }

    result
}

fn highlight_toml_yaml(line: &str) -> String {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return line.dimmed().to_string();
    }

    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return line.cyan().bold().to_string();
    }

    if let Some(colon_pos) = line.find(':') {
        let key = &line[..colon_pos];
        let val = &line[colon_pos + 1..];
        return format!("{}:{}", key.cyan().bold(), highlight_generic(val));
    }

    if let Some(eq_pos) = line.find('=') {
        let key = &line[..eq_pos];
        let val = &line[eq_pos + 1..];
        return format!("{}={}", key.cyan().bold(), highlight_generic(val));
    }

    highlight_generic(line)
}

fn highlight_generic(line: &str) -> String {
    let keywords = [
        "true", "false", "null", "if", "else", "function", "return", "let", "const", "def",
    ];
    let types = ["int", "string", "bool"];
    tokenize_and_color(line, &keywords, &types, "//", false)
}

/// Generic lexer tokenizer that identifies keywords, types, numbers, string literals, and comments.
fn tokenize_and_color(
    line: &str,
    keywords: &[&str],
    types: &[&str],
    comment_marker: &str,
    allow_decorators: bool,
) -> String {
    let mut result = String::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Comments
        let slice: String = chars[i..].iter().collect();
        if slice.starts_with(comment_marker) {
            let comment_text: String = chars[i..].iter().collect();
            result.push_str(&comment_text.dimmed().to_string());
            break;
        }

        // Decorator in Python / TypeScript (@decorator)
        if allow_decorators && chars[i] == '@' {
            let mut decorator = String::from("@");
            i += 1;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                decorator.push(chars[i]);
                i += 1;
            }
            result.push_str(&decorator.magenta().bold().to_string());
            continue;
        }

        // Shell variable ($VAR or ${VAR})
        if chars[i] == '$' {
            let mut var_str = String::from("$");
            i += 1;
            if i < len && chars[i] == '{' {
                var_str.push('{');
                i += 1;
                while i < len && chars[i] != '}' {
                    var_str.push(chars[i]);
                    i += 1;
                }
                if i < len && chars[i] == '}' {
                    var_str.push('}');
                    i += 1;
                }
            } else {
                while i < len && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '?')
                {
                    var_str.push(chars[i]);
                    i += 1;
                }
            }
            result.push_str(&var_str.yellow().bold().to_string());
            continue;
        }

        // String literals ("..." or '...')
        if chars[i] == '"' || chars[i] == '\'' || chars[i] == '`' {
            let quote = chars[i];
            let mut str_val = String::new();
            str_val.push(quote);
            i += 1;
            let mut escaped = false;

            while i < len {
                let c = chars[i];
                str_val.push(c);
                i += 1;
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == quote {
                    break;
                }
            }
            result.push_str(&str_val.green().to_string());
            continue;
        }

        // Numbers (decimal, hex 0x..., floats)
        if chars[i].is_ascii_digit()
            || (chars[i] == '.' && i + 1 < len && chars[i + 1].is_ascii_digit())
        {
            let mut num_str = String::new();
            while i < len
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                num_str.push(chars[i]);
                i += 1;
            }
            result.push_str(&num_str.yellow().to_string());
            continue;
        }

        // Identifiers / Words / Keywords / Types
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let mut word = String::new();
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                word.push(chars[i]);
                i += 1;
            }

            // Check if it's a macro invocation like println!
            let is_macro = i < len && chars[i] == '!';
            if is_macro {
                word.push('!');
                i += 1;
                result.push_str(&word.magenta().bold().to_string());
                continue;
            }

            if keywords.iter().any(|&k| k.eq_ignore_ascii_case(&word)) {
                result.push_str(&word.magenta().bold().to_string());
            } else if types.contains(&word.as_str()) {
                result.push_str(&word.cyan().bold().to_string());
            } else if i < len && chars[i] == '(' {
                // Function call: foo()
                result.push_str(&word.blue().to_string());
            } else {
                result.push_str(&word);
            }
            continue;
        }

        // Operators & Punctuation
        result.push(chars[i]);
        i += 1;
    }

    result
}

// -----------------------------------------------------------------------------
// Unit Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_tag_parsing() {
        assert_eq!(Language::from_tag("rust"), Language::Rust);
        assert_eq!(Language::from_tag("rs"), Language::Rust);
        assert_eq!(Language::from_tag("py"), Language::Python);
        assert_eq!(Language::from_tag("python"), Language::Python);
        assert_eq!(Language::from_tag("ts"), Language::TypeScript);
        assert_eq!(Language::from_tag("typescript"), Language::TypeScript);
        assert_eq!(Language::from_tag("js"), Language::JavaScript);
        assert_eq!(Language::from_tag("go"), Language::Go);
        assert_eq!(Language::from_tag("bash"), Language::Bash);
        assert_eq!(Language::from_tag("sh"), Language::Bash);
        assert_eq!(Language::from_tag("json"), Language::Json);
        assert_eq!(Language::from_tag("toml"), Language::Toml);
        assert_eq!(Language::from_tag("yaml"), Language::Yaml);
        assert_eq!(Language::from_tag("sql"), Language::Sql);
        assert_eq!(Language::from_tag("unknown_lang_xyz"), Language::Unknown);
    }

    #[test]
    fn test_highlight_rust_keywords_and_types() {
        let code = "pub fn calculate_sum(a: i32, b: i32) -> Result<i32, String> {";
        let highlighted = highlight_rust(code);
        assert!(!highlighted.is_empty());
        // Should contain ANSI escape codes
        assert!(highlighted.contains("\u{1b}["));
    }

    #[test]
    fn test_highlight_rust_comment() {
        let code = "let x = 42; // calculate total value";
        let highlighted = highlight_rust(code);
        assert!(highlighted.contains("// calculate total value"));
    }

    #[test]
    fn test_highlight_rust_string_and_macro() {
        let code = "println!(\"Hello, world! Answer: {}\", 42);";
        let highlighted = highlight_rust(code);
        assert!(highlighted.contains("println!"));
        assert!(highlighted.contains("\"Hello, world! Answer: {}\""));
    }

    #[test]
    fn test_highlight_python_def_and_decorator() {
        let code = "@app.get('/users')\ndef fetch_users(uid: str) -> dict:\n    # Return users\n    return {'uid': uid}";
        for line in code.lines() {
            let highlighted = highlight_python(line);
            assert!(!highlighted.is_empty());
        }
    }

    #[test]
    fn test_highlight_markdown_headers_and_spans() {
        let h1 = highlight_markdown_line("# Project Orbit");
        assert!(h1.contains("\u{1b}["));

        let bullet = highlight_markdown_line("- Support for `Cargo.toml` and **Rust**");
        assert!(bullet.contains("•"));
        assert!(bullet.contains("`Cargo.toml`"));

        let quote = highlight_markdown_line("> This is an important note");
        assert!(quote.contains("│"));
    }

    #[test]
    fn test_streaming_highlighter_feed_and_flush() {
        let mut highlighter = StreamingHighlighter::new(true);

        // Feed chunks split across lines and code blocks
        highlighter.feed("Here is the code:\n```rust\nfn main() {\n");
        assert!(highlighter.in_code_block);
        assert_eq!(highlighter.current_lang, Language::Rust);

        highlighter.feed("    let msg = \"Hello\";\n    println!(\"{}\", msg);\n}\n```\nDone!");
        assert!(!highlighter.in_code_block);

        highlighter.flush();
        assert!(!highlighter.in_code_block);
    }

    #[test]
    fn test_streaming_highlighter_unclosed_block_flush() {
        let mut highlighter = StreamingHighlighter::new(true);
        highlighter.feed("```python\ndef test():\n    pass");
        assert!(highlighter.in_code_block);
        highlighter.flush();
        // Flush should close unclosed blocks cleanly
        assert!(!highlighter.in_code_block);
    }
}
