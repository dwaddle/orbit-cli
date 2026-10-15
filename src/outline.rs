use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Class,
    Interface,
    TypeAlias,
    Module,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "fn",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Impl => "impl",
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::TypeAlias => "type",
            SymbolKind::Module => "mod",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
    pub signature: String,
}

#[derive(Debug, Clone)]
pub struct FileOutline {
    pub path: PathBuf,
    pub relative_path: String,
    pub symbols: Vec<Symbol>,
}

pub struct SymbolExtractor {
    rust_fn: Regex,
    rust_struct: Regex,
    rust_enum: Regex,
    rust_trait: Regex,
    rust_impl: Regex,
    rust_mod: Regex,
    rust_type: Regex,

    py_def: Regex,
    py_class: Regex,

    ts_func: Regex,
    ts_class: Regex,
    ts_interface: Regex,
    ts_type: Regex,

    go_func: Regex,
    go_type: Regex,
}

impl Default for SymbolExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolExtractor {
    pub fn new() -> Self {
        Self {
            rust_fn: Regex::new(r"^\s*(?:pub(?:\([^\)]+\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+([a-zA-Z0-9_]+)\s*(?:<[^>]+>)?\s*\((.*?)\)").unwrap(),
            rust_struct: Regex::new(r"^\s*(?:pub(?:\([^\)]+\))?\s+)?struct\s+([a-zA-Z0-9_]+)").unwrap(),
            rust_enum: Regex::new(r"^\s*(?:pub(?:\([^\)]+\))?\s+)?enum\s+([a-zA-Z0-9_]+)").unwrap(),
            rust_trait: Regex::new(r"^\s*(?:pub(?:\([^\)]+\))?\s+)?trait\s+([a-zA-Z0-9_]+)").unwrap(),
            rust_impl: Regex::new(r"^\s*impl(?:\s*<[^>]+>)?\s+(?:([a-zA-Z0-9_:]+)\s+for\s+)?([a-zA-Z0-9_:]+)").unwrap(),
            rust_mod: Regex::new(r"^\s*(?:pub(?:\([^\)]+\))?\s+)?mod\s+([a-zA-Z0-9_]+)\s*;").unwrap(),
            rust_type: Regex::new(r"^\s*(?:pub(?:\([^\)]+\))?\s+)?type\s+([a-zA-Z0-9_]+)").unwrap(),

            py_def: Regex::new(r"^\s*(?:async\s+)?def\s+([a-zA-Z0-9_]+)\s*\((.*?)\)").unwrap(),
            py_class: Regex::new(r"^\s*class\s+([a-zA-Z0-9_]+)(?:\((.*?)\))?").unwrap(),

            ts_func: Regex::new(r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+([a-zA-Z0-9_]+)").unwrap(),
            ts_class: Regex::new(r"^\s*(?:export\s+)?(?:default\s+)?class\s+([a-zA-Z0-9_]+)").unwrap(),
            ts_interface: Regex::new(r"^\s*(?:export\s+)?interface\s+([a-zA-Z0-9_]+)").unwrap(),
            ts_type: Regex::new(r"^\s*(?:export\s+)?type\s+([a-zA-Z0-9_]+)").unwrap(),

            go_func: Regex::new(r"^\s*func\s+(?:\([^\)]+\)\s+)?([a-zA-Z0-9_]+)\s*\(").unwrap(),
            go_type: Regex::new(r"^\s*type\s+([a-zA-Z0-9_]+)\s+(struct|interface)").unwrap(),
        }
    }

    pub fn extract_symbols(&self, content: &str, ext: &str) -> Vec<Symbol> {
        let mut symbols = Vec::new();

        for (idx, line) in content.lines().enumerate() {
            let line_num = idx + 1;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
                continue;
            }

            match ext {
                "rs" => {
                    if let Some(caps) = self.rust_fn.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::Function,
                            line: line_num,
                            signature: format!(
                                "{}({})",
                                &caps[1],
                                caps.get(2).map_or("", |m| m.as_str().trim())
                            ),
                        });
                    } else if let Some(caps) = self.rust_struct.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::Struct,
                            line: line_num,
                            signature: format!("struct {}", &caps[1]),
                        });
                    } else if let Some(caps) = self.rust_enum.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::Enum,
                            line: line_num,
                            signature: format!("enum {}", &caps[1]),
                        });
                    } else if let Some(caps) = self.rust_trait.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::Trait,
                            line: line_num,
                            signature: format!("trait {}", &caps[1]),
                        });
                    } else if let Some(caps) = self.rust_impl.captures(line) {
                        let trait_part = caps.get(1).map_or("", |m| m.as_str());
                        let target = &caps[2];
                        let sig = if trait_part.is_empty() {
                            format!("impl {}", target)
                        } else {
                            format!("impl {} for {}", trait_part, target)
                        };
                        symbols.push(Symbol {
                            name: target.to_string(),
                            kind: SymbolKind::Impl,
                            line: line_num,
                            signature: sig,
                        });
                    } else if let Some(caps) = self.rust_mod.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::Module,
                            line: line_num,
                            signature: format!("mod {}", &caps[1]),
                        });
                    } else if let Some(caps) = self.rust_type.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::TypeAlias,
                            line: line_num,
                            signature: format!("type {}", &caps[1]),
                        });
                    }
                }
                "py" => {
                    if let Some(caps) = self.py_def.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::Function,
                            line: line_num,
                            signature: format!(
                                "{}({})",
                                &caps[1],
                                caps.get(2).map_or("", |m| m.as_str().trim())
                            ),
                        });
                    } else if let Some(caps) = self.py_class.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::Class,
                            line: line_num,
                            signature: format!("class {}", &caps[1]),
                        });
                    }
                }
                "ts" | "tsx" | "js" | "jsx" => {
                    if let Some(caps) = self.ts_func.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::Function,
                            line: line_num,
                            signature: format!("fn {}", &caps[1]),
                        });
                    } else if let Some(caps) = self.ts_class.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::Class,
                            line: line_num,
                            signature: format!("class {}", &caps[1]),
                        });
                    } else if let Some(caps) = self.ts_interface.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::Interface,
                            line: line_num,
                            signature: format!("interface {}", &caps[1]),
                        });
                    } else if let Some(caps) = self.ts_type.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::TypeAlias,
                            line: line_num,
                            signature: format!("type {}", &caps[1]),
                        });
                    }
                }
                "go" => {
                    if let Some(caps) = self.go_func.captures(line) {
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind: SymbolKind::Function,
                            line: line_num,
                            signature: format!("func {}", &caps[1]),
                        });
                    } else if let Some(caps) = self.go_type.captures(line) {
                        let kind = if &caps[2] == "struct" {
                            SymbolKind::Struct
                        } else {
                            SymbolKind::Interface
                        };
                        symbols.push(Symbol {
                            name: caps[1].to_string(),
                            kind,
                            line: line_num,
                            signature: format!("type {} {}", &caps[1], &caps[2]),
                        });
                    }
                }
                _ => {}
            }
        }

        symbols
    }
}

pub fn is_home_or_root(dir: &Path) -> bool {
    if dir == Path::new("/") {
        return true;
    }
    if let Some(home) = dirs::home_dir() {
        if dir == home {
            return true;
        }
    }
    false
}

pub fn is_ignored_path(path: &Path) -> bool {
    if path.is_symlink() {
        return true;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.starts_with('.')
        || name == "target"
        || name == "node_modules"
        || name == "vendor"
        || name == ".git"
        || name == "dist"
        || name == "build"
        || name == "venv"
        || name == ".venv"
        || name == "__pycache__"
        || name == "pkg"
        || name == "out"
        || name == "cache"
}

/// Recursively traverses `root_dir` extracting symbol maps across project source files.
pub fn generate_codebase_outline(root_dir: &Path, max_tokens: usize) -> String {
    let extractor = SymbolExtractor::new();
    let mut files = Vec::new();
    collect_source_files(root_dir, root_dir, 0, &mut files);

    // Sort files alphabetically for stable output
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    let mut outlines = Vec::new();
    for file in files {
        if let Ok(content) = fs::read_to_string(&file.path) {
            let ext = file.path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let symbols = extractor.extract_symbols(&content, ext);
            if !symbols.is_empty() {
                outlines.push(FileOutline {
                    path: file.path,
                    relative_path: file.relative_path,
                    symbols,
                });
            }
        }
    }

    format_outline(&outlines, max_tokens)
}

struct SourceFileInfo {
    path: PathBuf,
    relative_path: String,
}

fn collect_source_files(
    current_dir: &Path,
    base_dir: &Path,
    depth: usize,
    out: &mut Vec<SourceFileInfo>,
) {
    if out.len() >= 300 {
        return;
    }
    let max_depth = if is_home_or_root(base_dir) { 1 } else { 6 };
    if depth > max_depth {
        return;
    }

    let entries = match fs::read_dir(current_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if out.len() >= 300 {
            break;
        }
        let path = entry.path();
        if is_ignored_path(&path) {
            continue;
        }

        if path.is_dir() {
            if is_home_or_root(base_dir) && depth >= 1 {
                continue;
            }
            collect_source_files(&path, base_dir, depth + 1, out);
        } else if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go") {
                let relative = path
                    .strip_prefix(base_dir)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());

                out.push(SourceFileInfo {
                    path,
                    relative_path: relative,
                });
            }
        }
    }
}

pub fn format_outline(outlines: &[FileOutline], max_tokens: usize) -> String {
    let max_chars = max_tokens * 4;
    let mut buffer = String::from("## CODEBASE SYMBOL MAP & ARCHITECTURE OUTLINE\n\n");

    if outlines.is_empty() {
        buffer.push_str("No symbol definitions extracted.\n");
        return buffer;
    }

    for file in outlines {
        let file_header = format!("• `{}`:\n", file.relative_path);
        if buffer.len() + file_header.len() > max_chars {
            buffer.push_str("\n... [Remaining files truncated to conserve context]\n");
            break;
        }
        buffer.push_str(&file_header);

        for sym in &file.symbols {
            let sym_line = format!(
                "  - [{}] {} (L{})\n",
                sym.kind.as_str(),
                sym.signature,
                sym.line
            );
            if buffer.len() + sym_line.len() > max_chars {
                buffer.push_str("  - ... [symbols truncated]\n");
                break;
            }
            buffer.push_str(&sym_line);
        }
    }

    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rust_symbols() {
        let code = r#"
            pub struct AgentConfig {
                pub id: String,
            }

            pub enum Status {
                Active,
                Idle,
            }

            pub trait Runner {
                fn run(&self);
            }

            impl Runner for AgentConfig {
                fn run(&self) {}
            }

            pub async fn execute_task(task_id: usize) -> bool {
                true
            }
        "#;

        let extractor = SymbolExtractor::new();
        let symbols = extractor.extract_symbols(code, "rs");

        assert_eq!(symbols.len(), 7);
        assert_eq!(symbols[0].name, "AgentConfig");
        assert_eq!(symbols[0].kind, SymbolKind::Struct);
        assert_eq!(symbols[1].name, "Status");
        assert_eq!(symbols[1].kind, SymbolKind::Enum);
        assert_eq!(symbols[2].name, "Runner");
        assert_eq!(symbols[2].kind, SymbolKind::Trait);
        assert_eq!(symbols[3].name, "run");
        assert_eq!(symbols[3].kind, SymbolKind::Function);
        assert_eq!(symbols[4].name, "AgentConfig");
        assert_eq!(symbols[4].kind, SymbolKind::Impl);
        assert_eq!(symbols[5].name, "run");
        assert_eq!(symbols[5].kind, SymbolKind::Function);
        assert_eq!(symbols[6].name, "execute_task");
        assert_eq!(symbols[6].kind, SymbolKind::Function);
    }

    #[test]
    fn test_extract_python_symbols() {
        let code = r#"
            class DatabaseManager:
                def __init__(self, uri: str):
                    self.uri = uri

                async def connect(self):
                    pass
        "#;

        let extractor = SymbolExtractor::new();
        let symbols = extractor.extract_symbols(code, "py");

        assert_eq!(symbols.len(), 3);
        assert_eq!(symbols[0].name, "DatabaseManager");
        assert_eq!(symbols[0].kind, SymbolKind::Class);
        assert_eq!(symbols[1].name, "__init__");
        assert_eq!(symbols[2].name, "connect");
    }

    #[test]
    fn test_generate_outline_bounded_length() {
        let outlines = vec![FileOutline {
            path: PathBuf::from("src/test.rs"),
            relative_path: "src/test.rs".to_string(),
            symbols: vec![Symbol {
                name: "test_fn".to_string(),
                kind: SymbolKind::Function,
                line: 10,
                signature: "test_fn()".to_string(),
            }],
        }];

        let formatted = format_outline(&outlines, 100);
        assert!(formatted.contains("src/test.rs"));
        assert!(formatted.contains("test_fn()"));
        assert!(formatted.len() <= 400);
    }
}
