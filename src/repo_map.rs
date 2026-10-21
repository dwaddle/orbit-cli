use anyhow::Result;
use ignore::WalkBuilder;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SymbolDef {
    pub name: String,
    pub kind: String, // "fn", "struct", "enum", "class", "trait", "interface"
    pub signature: String,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct FileAstInfo {
    pub path: PathBuf,
    pub rel_path: String,
    pub symbols: Vec<SymbolDef>,
    pub references: HashSet<String>,
}

pub struct RepoMap;

impl RepoMap {
    pub fn generate(root: &Path, max_lines: usize) -> Result<String> {
        let files = Self::collect_and_parse(root)?;
        if files.is_empty() {
            return Ok("No source code files found in workspace.".to_string());
        }

        // Calculate PageRank scores
        let scores = Self::compute_pagerank(&files);

        // Sort files by score descending
        let mut sorted_files = files;
        sorted_files.sort_by(|a, b| {
            let score_a = scores.get(&a.rel_path).copied().unwrap_or(0.0);
            let score_b = scores.get(&b.rel_path).copied().unwrap_or(0.0);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Format into compact repo-map
        let mut lines = Vec::new();
        lines.push("🗺️ === REPO MAP (AST Architecture Skelet) ===".to_string());

        let mut lines_used = 1;
        for file in sorted_files {
            if lines_used >= max_lines {
                break;
            }

            if file.symbols.is_empty() {
                continue;
            }

            let score = scores.get(&file.rel_path).copied().unwrap_or(0.0);
            lines.push(format!(
                "\n📄 {} (rank: {:.2}):",
                file.rel_path,
                score * 100.0
            ));
            lines_used += 2;

            for sym in &file.symbols {
                if lines_used >= max_lines {
                    lines.push(
                        "  ... (more symbols truncated for context token budget)".to_string(),
                    );
                    lines_used += 1;
                    break;
                }

                let sig = if sym.signature.len() > 80 {
                    format!("{}...", &sym.signature[..77])
                } else {
                    sym.signature.clone()
                };

                lines.push(format!("  {} {}:{}", sym.kind, sym.name, sig));
                lines_used += 1;
            }
        }

        Ok(lines.join("\n"))
    }

    fn collect_and_parse(root: &Path) -> Result<Vec<FileAstInfo>> {
        let walker = WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .parents(true)
            .build();

        let mut out = Vec::new();

        let rust_sym_re = Regex::new(
            r"(?m)^\s*(?:pub(?:\([^\)]+\))?\s+)?(fn|struct|enum|trait|type|impl)\s+([A-Za-z0-9_]+)(?:<[^>]+>)?\s*([^{;]*)"
        ).unwrap();

        let py_sym_re =
            Regex::new(r"(?m)^\s*(def|class)\s+([A-Za-z0-9_]+)\s*(\([^\)]*\))?").unwrap();

        let ts_sym_re = Regex::new(
            r"(?m)^\s*(?:export\s+)?(function|class|interface|type|enum|const)\s+([A-Za-z0-9_]+)\s*([^{;=]*)"
        ).unwrap();

        let ident_re = Regex::new(r"\b[A-Za-z_][A-Za-z0-9_]{2,}\b").unwrap();

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext = match path.extension().and_then(|s| s.to_str()) {
                Some(e) => e.to_lowercase(),
                None => continue,
            };

            let is_supported = matches!(ext.as_str(), "rs" | "py" | "ts" | "js" | "go");
            if !is_supported {
                continue;
            }

            let content = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let mut symbols = Vec::new();
            match ext.as_str() {
                "rs" => {
                    for cap in rust_sym_re.captures_iter(&content) {
                        let kind = cap.get(1).map_or("", |m| m.as_str());
                        let name = cap.get(2).map_or("", |m| m.as_str());
                        let sig = cap.get(3).map_or("", |m| m.as_str()).trim();
                        symbols.push(SymbolDef {
                            name: name.to_string(),
                            kind: kind.to_string(),
                            signature: sig.to_string(),
                            line: 0,
                        });
                    }
                }
                "py" => {
                    for cap in py_sym_re.captures_iter(&content) {
                        let kind = cap.get(1).map_or("", |m| m.as_str());
                        let name = cap.get(2).map_or("", |m| m.as_str());
                        let sig = cap.get(3).map_or("", |m| m.as_str()).trim();
                        symbols.push(SymbolDef {
                            name: name.to_string(),
                            kind: kind.to_string(),
                            signature: sig.to_string(),
                            line: 0,
                        });
                    }
                }
                "ts" | "js" => {
                    for cap in ts_sym_re.captures_iter(&content) {
                        let kind = cap.get(1).map_or("", |m| m.as_str());
                        let name = cap.get(2).map_or("", |m| m.as_str());
                        let sig = cap.get(3).map_or("", |m| m.as_str()).trim();
                        symbols.push(SymbolDef {
                            name: name.to_string(),
                            kind: kind.to_string(),
                            signature: sig.to_string(),
                            line: 0,
                        });
                    }
                }
                _ => {}
            }

            let mut references = HashSet::new();
            for m in ident_re.find_iter(&content) {
                references.insert(m.as_str().to_string());
            }

            out.push(FileAstInfo {
                path: path.to_path_buf(),
                rel_path,
                symbols,
                references,
            });
        }

        Ok(out)
    }

    fn compute_pagerank(files: &[FileAstInfo]) -> HashMap<String, f64> {
        let n = files.len();
        if n == 0 {
            return HashMap::new();
        }

        // Map symbol names to the files that define them
        let mut def_map: HashMap<String, Vec<String>> = HashMap::new();
        for file in files {
            for sym in &file.symbols {
                def_map
                    .entry(sym.name.clone())
                    .or_default()
                    .push(file.rel_path.clone());
            }
        }

        // Build directed adjacency list: caller -> callee files
        let mut outgoing: HashMap<String, HashSet<String>> = HashMap::new();
        let mut incoming: HashMap<String, Vec<String>> = HashMap::new();

        for file in files {
            outgoing.entry(file.rel_path.clone()).or_default();
            incoming.entry(file.rel_path.clone()).or_default();

            for ref_ident in &file.references {
                if let Some(target_files) = def_map.get(ref_ident) {
                    for target in target_files {
                        if target != &file.rel_path {
                            outgoing
                                .entry(file.rel_path.clone())
                                .or_default()
                                .insert(target.clone());
                            incoming
                                .entry(target.clone())
                                .or_default()
                                .push(file.rel_path.clone());
                        }
                    }
                }
            }
        }

        // Standard PageRank iteration
        let damping = 0.85;
        let mut rank: HashMap<String, f64> = files
            .iter()
            .map(|f| (f.rel_path.clone(), 1.0 / n as f64))
            .collect();

        for _ in 0..10 {
            let mut next_rank = HashMap::new();
            let base_rank = (1.0 - damping) / n as f64;

            for file in files {
                let mut sum = 0.0;
                if let Some(in_nodes) = incoming.get(&file.rel_path) {
                    for in_node in in_nodes {
                        let out_deg = outgoing.get(in_node).map(|s| s.len()).unwrap_or(0);
                        if out_deg > 0 {
                            let r = rank.get(in_node).copied().unwrap_or(0.0);
                            sum += r / out_deg as f64;
                        }
                    }
                }
                next_rank.insert(file.rel_path.clone(), base_rank + damping * sum);
            }
            rank = next_rank;
        }

        rank
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_repo_map_generation() -> Result<()> {
        let temp = tempdir()?;
        let mod_a = temp.path().join("core.rs");
        let mod_b = temp.path().join("service.rs");

        fs::write(
            &mod_a,
            "pub struct DatabaseClient;\npub fn connect() -> DatabaseClient { DatabaseClient }\n",
        )?;
        fs::write(
            &mod_b,
            "use crate::core::DatabaseClient;\npub fn run_service() {\n    let _ = DatabaseClient;\n}\n",
        )?;

        let map = RepoMap::generate(temp.path(), 50)?;
        assert!(map.contains("REPO MAP"));
        assert!(map.contains("core.rs"));
        assert!(map.contains("DatabaseClient"));
        assert!(map.contains("service.rs"));

        Ok(())
    }
}
