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
- **🌐 OpenRouter Multi-Model Flexibility**: Dynamically query, search, and switch between state-of-the-art models (`claude-sonnet-4.5`, `deepseek-r1`, `gpt-4o`, `gemini-2.5-pro`, `qwen`, etc.) via `/model`.
- **⚡ Real-Time Streaming**: Low-latency token streaming using Server-Sent Events (SSE).
- **🛠️ 7 Autonomous Built-in Tools**:
  - `run_command`: Local shell execution and stdout/stderr capture.
  - `view_file`: Workspace file exploration.
  - `write_file`: In-place code generation and editing.
  - `code_search`: BM25 keyword search across workspace code.
  - `code_outline`: Symbol map extraction for files or directories.
  - `git_context`: Log, blame, diff, and pickaxe search.
  - `invoke_subagent`: Background subagent task spawning.
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

---

## ⌨️ Slash Commands

| Command | Description |
|---|---|
| `/search <query>` | Search workspace code using in-memory BM25 index |
| `/outline [path]` | Generate codebase symbol outline (Rust, Python, TS/JS, Go) |
| `/git <subcmd> [args]` | Inspect git context (`log`, `blame`, `diff`, `search`) |
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
