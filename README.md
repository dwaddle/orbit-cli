# 🛰️ Orbit CLI (`orbit-cli`)

[![Rust](https://img.shields.io/badge/rust-2021_edition-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![OpenRouter](https://img.shields.io/badge/Powered%20By-OpenRouter-7928CA.svg)](https://openrouter.ai/)

**Orbit** is a lightweight, high-performance autonomous AI coding agent in Rust. Powered by OpenRouter's multi-model API, Orbit delivers real-time token streaming, Model Context Protocol (MCP) integration, subagent task delegation, and direct local workspace tool execution.

---

## ✨ Features

- **🚀 Ultra-Fast Rust Runtime**: Single native binary with minimal memory footprint and instant startup.
- **🌐 OpenRouter Multi-Model Flexibility**: Dynamically query, search, and switch between state-of-the-art models (`claude-3.7-sonnet`, `claude-3.5-sonnet`, `deepseek-r1`, `gpt-4o`, `gemini-2.5-pro`, `qwen`, and more) via `/model`.
- **⚡ Real-Time Streaming**: Low-latency token streaming using Server-Sent Events (SSE).
- **🛠️ Autonomous Tool Calling**:
  - `run_command`: Safe local bash execution and capture.
  - `view_file`: Workspace file exploration.
  - `write_file`: In-place code generation and editing.
- **🔌 Model Context Protocol (MCP)**: Native client support for external MCP servers, discovering and routing specialized tool schemas.
- **🤖 Subagent Task Delegation**: Spawn background subagents to work concurrently on research, audits, or isolated subtasks.
- **💬 Interactive REPL**: Built with `rustyline` for command history, input editing, and slash command workflows.

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

2. **Launch Orbit**:
   ```bash
   orbit-cli
   ```

3. **Start pair programming**:
   ```text
   orbit > Analyse the current git repository and list all modified files.
   ```

---

## ⌨️ Slash Commands

| Command | Description |
|---|---|
| `/model` | List top available models and display current selection |
| `/model <search|id>` | Search and switch active OpenRouter model |
| `/subagents` | Inspect running and completed subagent tasks |
| `/mcp` | List loaded MCP servers and available tools |
| `/file <path>` | Inject an entire file into the conversation context |
| `/tools` | Toggle autonomous tool execution ON / OFF |
| `/clear` | Clear active conversation history |
| `/key <key>` | Update OpenRouter API key |
| `/help` | Display command help |
| `/exit` | Exit the CLI |

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
