# 🛰️ Orbit CLI (`orbit-cli`)

[![Rust](https://img.shields.io/badge/rust-2021_edition-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/dwaddle/orbit-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/dwaddle/orbit-cli/actions/workflows/ci.yml)
[![OpenRouter](https://img.shields.io/badge/Powered%20By-OpenRouter-7928CA.svg)](https://openrouter.ai/)

**Orbit** is a lightweight, high-performance autonomous AI coding agent in Rust. Powered by OpenRouter's multi-model API, Orbit delivers real-time token streaming, Model Context Protocol (MCP) integration, subagent task delegation, multi-language codebase indexing, git intelligence, and both a terminal REPL and a fullscreen split-pane Ratatui TUI.

```text
  ___   ____  ____  ___ _____     ____ _     ___
 / _ \ |  _ \| __ )|_ _|_   _|   / ___| |   |_ _|
| | | || |_) |  _ \ | |  | |    | |   | |    | |
| |_| ||  _ <| |_) || |  | |    | |___| |___ | |
 \___/ |_| \_\____/|___| |_|     \____|_____|___|

Loaded 7 built-in workspace tools: [run_command, view_file, write_file, code_search, code_outline, git_context, invoke_subagent]
Connected to OpenRouter | Active Model: anthropic/claude-sonnet-4.5

orbit >
```

---

## ✨ Features

- **🚀 Ultra-Fast Rust Runtime**: Single native binary with minimal memory footprint and instant startup.
- **🖥️ Fullscreen Split TUI (Ratatui)**: Fullscreen split-pane interface featuring a live workspace file-tree browser, active subagent task monitor, streaming chat, and interactive prompt (`--tui` or `/tui`).
- **🔍 Inverted BM25 Code Search**: In-memory keyword search index with camelCase/snake_case tokenization and lazy, bounded indexing (`/search` and `code_search`).
- **🗺️ Codebase Symbol Outline**: Multi-language symbol extractor for Rust, Python, TypeScript/JavaScript, and Go (`/outline` and `code_outline`).
- **📜 Git Context Intelligence**: Built-in git history inspection with commit logs, line-range blame, unified diffs, and pickaxe search (`/git` and `git_context`).
- **🧠 Sliding-Window Context & Tool Compression**: Smart token budgeting that compresses large tool outputs (>150 lines or >12,000 chars) and summarizes older turns to prevent context window overflow.
- **📋 Project Rules Auto-Discovery**: Hierarchical upward search for `.orbitrules`, `AGENTS.md`, `.agents/AGENTS.md`, and `.orbit/rules.md`, automatically injected into the system prompt.
- **💰 OpenRouter Prompt Caching**: Injects explicit ephemeral `cache_control` headers into static system prompts, project rules, and reference files, cutting token costs by up to 90% on Claude and DeepSeek.
- **⚡ Hybrid Model Cascading & Auto-Routing**: Dynamically routes exploration, inspection, and planning queries to fast models (`gemini-2.5-flash`), escalating to heavy reasoners (`claude-sonnet-4.5`, `deepseek-r1`) for implementation and synthesis (`/cascade`).
- **🎯 Search-and-Replace Surgical Patching (Aider-style)**: Targeted `<<<<<<< SEARCH / ======= / >>>>>>> REPLACE` in-place patching that saves 80-90% tokens and eliminates whole-file hallucination regressions (`apply_patch`).
- **🛡️ AST Compile Guard & Self-Healing Loop**: Real-time post-edit compiler check (`cargo check`, `tsc --noEmit`, `py_compile`, `go vet`) with autonomous error diagnostic feedback to self-heal code errors before user review (`/guard`, `/heal`, `/compile`).
- **⏪ Instant Checkpoint & Time-Machine Rollback**: Automatic in-memory snapshot captured before any file modification, enabling instant single-command rollback (`/undo` and `/checkpoints`).
- **🧪 Deterministic Test-Driven Development Loop**: Autonomous Red -> Green -> Refactor cycle driven by the local test runner (`/tdd` and `tdd_cycle`).
- **🕸️ Subagent Directed Acyclic Graph (DAG) Orchestration**: Parallel execution of multi-agent dependency graphs via Tokio channels, with upstream task outputs automatically injected into downstream prompts (`/dag audit`, `/dag feature`, or `run_dag`).
- **🛡️ Dual-Agent Adversarial Review**: Independent critique subagent evaluating proposed code and git diffs across 5 axes (correctness, security vulnerabilities, error handling, test coverage, and performance) with an optional automated post-write safety hook (`/review`, `/adversarial`, or `adversarial_review`).
- **🔭 Background File Watcher & Test Loop**: Cross-platform filesystem monitoring via `notify`, 400ms debounced test runner (supporting `cargo test`, `pytest`, `bun/npm test`, and `go test`), with instantaneous pass/fail diagnostics (`orbit-cli watch` or `/watch`).
- **🛡️ Provider Outage & Rate-Limit Auto-Failover**: Resilient multi-model retry chain that recovers from HTTP 429 rate limits or 5xx outages without dropping conversation context (`/fallback`).
- **🌐 OpenRouter Multi-Model Flexibility**: Dynamically query, search, and switch between state-of-the-art models (`claude-sonnet-4.5`, `deepseek-r1`, `gpt-4o`, `gemini-2.5-pro`, `qwen`, etc.) via `/model`.
- **⚡ Real-Time Streaming**: Low-latency token streaming using Server-Sent Events (SSE).
- **🛠️ 11 Autonomous Built-in Tools**:
  - `run_command`: Local shell execution and stdout/stderr capture.
  - `view_file`: Workspace file exploration.
  - `write_file`: In-place code generation and editing.
  - `apply_patch`: Aider-style surgical search/replace block patcher.
  - `tdd_cycle`: Autonomous Red -> Green -> Refactor test-driven loop.
  - `code_search`: BM25 keyword search across workspace code.
  - `code_outline`: Symbol map extraction for files or directories.
  - `git_context`: Log, blame, diff, and pickaxe search.
  - `invoke_subagent`: Background subagent task spawning.
  - `adversarial_review`: Ruthless 5-axis security and regression critique.
  - `run_dag`: Concurrent multi-agent workflow execution.
- **🔌 Model Context Protocol (MCP)**: Native client support for external MCP servers, discovering and routing specialized tool schemas.
- **💬 Interactive REPL**: Built with `rustyline` for command history, line editing, and slash command workflows.

---

## 📦 Installation & Build

### Prerequisites
- [Rust & Cargo](https://rustup.rs/) (edition 2021 or newer)
- An [OpenRouter API Key](https://openrouter.ai/keys)

### Build from source
```bash
git clone https://github.com/dwaddle/orbit-cli.git
cd orbit-cli
cargo build --release
```

The compiled binary will be located at `target/release/orbit-cli`.

To install it directly into your local cargo path (`~/.cargo/bin`):
```bash
cargo install --path .
```

---

## 🚀 Quick Start

1. **Set your OpenRouter API key**:
   ```bash
   export OPENROUTER_API_KEY="sk-or-v1-..."
   ```
   *(Alternatively, Orbit will prompt you on first launch and store it securely in `~/.config/openrouter/config.json`)*

2. **Launch Orbit in REPL mode**:
   ```bash
   orbit-cli
   ```

3. **Or launch Orbit in fullscreen TUI mode**:
   ```bash
   orbit-cli --tui
   ```

4. **Or launch the background test runner & file watcher**:
   ```bash
   orbit-cli watch
   ```

---

## ⌨️ Slash Commands

| Command | Description |
|---|---|
| `/search <query>` | Search workspace code using in-memory BM25 index |
| `/outline [path]` | Generate codebase symbol outline (Rust, Python, TS/JS, Go) |
| `/git <subcmd> [args]` | Inspect git context (`log`, `blame`, `diff`, `search`) |
| `/undo` | Instant rollback of last file edit to previous checkpoint |
| `/checkpoints` | List recent file checkpoints and snapshot stack |
| `/compile` | Run instant workspace AST compile & type check |
| `/guard` | Toggle AST Compile Guard (auto cargo check) ON / OFF |
| `/heal` | Toggle compiler diagnostic self-healing loop ON / OFF |
| `/tdd <spec>` | Autonomous Red -> Green -> Refactor test-driven development loop |
| `/dag <audit\|feature>` | Run parallel multi-agent DAG workflow with dependency aggregation |
| `/review [file\|diff]` | Run independent 5-axis adversarial security and quality review |
| `/adversarial` | Toggle automatic adversarial review on file writes ON / OFF |
| `/watch [start\|stop\|run]` | Manage background file watcher and debounced test loop |
| `/cache` | Toggle prompt caching headers ON / OFF |
| `/cascade` | Toggle hybrid cascading auto-routing ON / OFF |
| `/routing` | Display active routing tiers, caching, and failover routes |
| `/fallback <models>` | Set comma-separated failover model chain |
| `/tui` | Switch into fullscreen Ratatui TUI mode |
| `/model` | List top available models and display current selection |
| `/model <search\|id>` | Search and switch active OpenRouter model |
| `/subagents` | Inspect running and completed subagent tasks |
| `/mcp` | List loaded MCP servers and available tools |
| `/file <path>` | Inject an entire file into the conversation context |
| `/tools` | Toggle autonomous tool execution ON / OFF |
| `/clear` | Clear active conversation history |
| `/key <key>` | Update OpenRouter API key |
| `/help` | Display command help |
| `/exit` | Exit the CLI |

---

## 🖥️ Ratatui TUI Navigation

In fullscreen TUI mode (`orbit-cli --tui` or `/tui`):
- `[Tab]`: Cycle active pane focus (`Input` -> `File Tree` -> `Subagents` -> `Chat`).
- `[Up]` / `[Down]`: Scroll through chat history, files, or subagents.
- `[Enter]`: Submit message from input prompt.
- `[Esc]` or `[Ctrl+C]`: Exit TUI mode back to terminal.

---

## 🧩 Model Context Protocol (MCP) Configuration

Orbit loads MCP servers defined in your local config (`~/.config/openrouter/mcp.json` or `~/.config/orbit/mcp.json`):

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/workspace"]
    }
  }
}
```

---

## 📄 License

Licensed under the [MIT License](LICENSE).
