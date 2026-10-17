use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap},
    Terminal,
};
use std::io::{self, Stdout};
use std::path::Path;

use crate::modal_prompt::{is_line_continuation, strip_line_continuation};
use crate::outline::{is_home_or_root, is_ignored_path};
use crate::subagent::SubagentStatus;
use crate::AgentState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiMode {
    Normal,
    Insert,
    Visual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
    Input,
    FileTree,
    Subagents,
    Chat,
}

pub struct TuiApp {
    pub active_pane: ActivePane,
    pub mode: TuiMode,
    pub input_text: String,
    pub file_tree: Vec<String>,
    pub selected_file: usize,
    pub chat_scroll: u16,
    pub max_chat_scroll: u16,
    pub auto_scroll: bool,
    pub streaming_reply: String,
    pub should_quit: bool,
    pub status_message: String,
    pub pending_key: Option<char>,
    pub visual_start: usize,
    pub visual_cursor: usize,
    pub yank_buffer: String,
}

impl TuiApp {
    pub fn new(workspace_dir: &Path) -> Self {
        let mut file_tree = Vec::new();
        collect_file_tree(workspace_dir, workspace_dir, 0, &mut file_tree);
        file_tree.sort();

        Self {
            active_pane: ActivePane::Input,
            mode: TuiMode::Insert,
            input_text: String::new(),
            file_tree,
            selected_file: 0,
            chat_scroll: 0,
            max_chat_scroll: 0,
            auto_scroll: true,
            streaming_reply: String::new(),
            should_quit: false,
            status_message:
                "Vim mode active. [Esc] Normal mode | [i] Insert | [Tab] Switch pane | [q] Quit"
                    .to_string(),
            pending_key: None,
            visual_start: 0,
            visual_cursor: 0,
            yank_buffer: String::new(),
        }
    }

    pub fn cycle_pane(&mut self) {
        self.active_pane = match self.active_pane {
            ActivePane::Input => ActivePane::FileTree,
            ActivePane::FileTree => ActivePane::Subagents,
            ActivePane::Subagents => ActivePane::Chat,
            ActivePane::Chat => ActivePane::Input,
        };
        if self.active_pane != ActivePane::Input {
            self.mode = TuiMode::Normal;
        }
    }

    pub fn cycle_pane_reverse(&mut self) {
        self.active_pane = match self.active_pane {
            ActivePane::Input => ActivePane::Chat,
            ActivePane::Chat => ActivePane::Subagents,
            ActivePane::Subagents => ActivePane::FileTree,
            ActivePane::FileTree => ActivePane::Input,
        };
        if self.active_pane != ActivePane::Input {
            self.mode = TuiMode::Normal;
        }
    }
}

fn collect_file_tree(current_dir: &Path, base_dir: &Path, depth: usize, out: &mut Vec<String>) {
    if out.len() >= 150 {
        return;
    }
    let max_depth = if is_home_or_root(base_dir) { 1 } else { 3 };
    if depth > max_depth {
        return;
    }

    let entries = match std::fs::read_dir(current_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if out.len() >= 150 {
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
            collect_file_tree(&path, base_dir, depth + 1, out);
        } else if path.is_file() {
            let relative = path
                .strip_prefix(base_dir)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());
            out.push(relative);
        }
    }
}

/// Runs the fullscreen Ratatui TUI mode for Orbit CLI.
pub async fn run_tui(state: &mut AgentState) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::cursor::Hide,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = TuiApp::new(&state.workspace_dir);

    let res = tui_event_loop(&mut terminal, &mut app, state).await;

    // Restore terminal safely
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    terminal.show_cursor()?;

    res
}

async fn submit_input(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut TuiApp,
    state: &mut AgentState,
) -> Result<()> {
    let input = app.input_text.trim().to_string();
    if input.is_empty() {
        return Ok(());
    }
    app.input_text.clear();

    if input == "/exit" || input == "/quit" {
        app.should_quit = true;
        return Ok(());
    } else if input == "/clear" {
        state.history.clear();
        app.streaming_reply.clear();
        app.chat_scroll = 0;
        app.auto_scroll = true;
        app.status_message = "Chat history cleared.".to_string();
        return Ok(());
    } else if input == "/mcp" {
        let mut msg = format!(
            "=== LOADED MCP SERVERS ({}) & TOOLS ({}) ===\n",
            state.mcp_clients.len(),
            state.mcp_tools.len()
        );
        if state.mcp_tools.is_empty() {
            msg.push_str("No MCP servers loaded. Add config to ~/.config/openrouter/mcp.json");
        } else {
            for client in &state.mcp_clients {
                let server_tools: Vec<&crate::mcp::McpTool> = state
                    .mcp_tools
                    .iter()
                    .filter(|t| t.server_name == client.server_name)
                    .collect();
                msg.push_str(&format!(
                    "\n[{}] ({} tools):\n",
                    client.server_name,
                    server_tools.len()
                ));
                for t in server_tools {
                    msg.push_str(&format!(
                        "  • {} — {}\n",
                        t.name,
                        t.description.as_deref().unwrap_or("")
                    ));
                }
            }
        }
        state.history.push(crate::Message {
            role: "system".to_string(),
            content: msg.trim_end().to_string(),
        });
        app.auto_scroll = true;
        app.status_message = format!(
            "Listed {} MCP tools across {} servers.",
            state.mcp_tools.len(),
            state.mcp_clients.len()
        );
        return Ok(());
    } else if input == "/tools" {
        state.config.tools_enabled = !state.config.tools_enabled;
        let _ = state.save_config();
        let status = if state.config.tools_enabled {
            "ENABLED"
        } else {
            "DISABLED"
        };
        state.history.push(crate::Message {
            role: "system".to_string(),
            content: format!("Tools auto-execution set to: {}", status),
        });
        app.auto_scroll = true;
        return Ok(());
    } else if input == "/help" {
        state.history.push(crate::Message {
            role: "system".to_string(),
            content: "Orbit TUI Vim Keybindings:\n\
                      - Normal Mode: [i]/[a] Insert mode | [j]/[k] Scroll/Move | [h]/[l] or [Tab] Switch Panes\n\
                      - Motions: [gg] Jump to top | [G] Jump to bottom | [Ctrl-d]/[Ctrl-u] Half page | [q] Quit\n\
                      - Insert Mode: Type prompt | [Enter] Submit | [Esc] or [Ctrl-[] Normal mode\n\
                      - Commands: /mcp (list MCP servers & tools), /tools (toggle tools), /clear (clear history), /help (help), /exit or /quit (quit TUI)".to_string(),
        });
        app.auto_scroll = true;
        return Ok(());
    }

    state.history.push(crate::Message {
        role: "user".to_string(),
        content: input,
    });

    let tool_re = regex::Regex::new(crate::TOOL_REGEX_PATTERN)?;

    for _step in 0..8 {
        let active_limit = state
            .available_models
            .iter()
            .find(|m| m.id == state.config.active_model)
            .and_then(|m| m.context_length)
            .map(|c| c as usize)
            .unwrap_or(crate::context::DEFAULT_CONTEXT_LIMIT);

        let sys_prompt = state.system_prompt();
        crate::context::auto_prune_context(
            &mut state.history,
            &sys_prompt,
            active_limit,
            crate::context::DEFAULT_PRUNE_THRESHOLD,
        );

        app.auto_scroll = true;
        app.streaming_reply.clear();
        app.status_message = format!("Generating response with {}...", state.config.active_model);
        terminal.draw(|frame| render_ui(frame, app, state))?;

        let full_result = {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let stream_fut = crate::stream_completion_channel(state, tx);
            tokio::pin!(stream_fut);

            let mut res: Option<Result<String>> = None;
            while res.is_none() {
                tokio::select! {
                    result = &mut stream_fut => {
                        res = Some(result);
                    }
                    Some(chunk) = rx.recv() => {
                        app.streaming_reply.push_str(&chunk);
                        terminal.draw(|frame| render_ui(frame, app, state))?;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                        if event::poll(std::time::Duration::from_millis(0))? {
                            if let Event::Key(key) = event::read()? {
                                if key.kind == KeyEventKind::Press && (key.code == KeyCode::Esc || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))) {
                                    app.status_message = "Generation interrupted by user.".to_string();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            res
        };

        match full_result {
            Some(Ok(full_reply)) => {
                state.history.push(crate::Message {
                    role: "assistant".to_string(),
                    content: full_reply.clone(),
                });
                app.streaming_reply.clear();
                terminal.draw(|frame| render_ui(frame, app, state))?;

                if state.config.tools_enabled {
                    let mut tool_executed = false;
                    for cap in tool_re.captures_iter(&full_reply) {
                        let tool_name = &cap[1];
                        let tool_body = &cap[2];
                        app.status_message = format!("⚙️ Executing tool: {}...", tool_name);
                        terminal.draw(|frame| render_ui(frame, app, state))?;

                        let tool_result =
                            crate::execute_tool_quiet(tool_name, tool_body, state).await;
                        state.history.push(crate::Message {
                            role: "user".to_string(),
                            content: format!("[TOOL OUTPUT ({})]:\n{}", tool_name, tool_result),
                        });
                        tool_executed = true;
                    }
                    if tool_executed {
                        terminal.draw(|frame| render_ui(frame, app, state))?;
                        continue;
                    }
                }

                app.mode = TuiMode::Insert;
                app.status_message =
                    "-- INSERT -- Ready. Type prompt | [Esc] Normal mode | [Enter] Submit"
                        .to_string();
                break;
            }
            Some(Err(e)) => {
                app.status_message = format!("Error: {}", e);
                if !app.streaming_reply.is_empty() {
                    state.history.push(crate::Message {
                        role: "assistant".to_string(),
                        content: std::mem::take(&mut app.streaming_reply),
                    });
                }
                break;
            }
            None => {
                if !app.streaming_reply.is_empty() {
                    state.history.push(crate::Message {
                        role: "assistant".to_string(),
                        content: std::mem::take(&mut app.streaming_reply),
                    });
                }
                break;
            }
        }
    }
    terminal.draw(|frame| render_ui(frame, app, state))?;
    Ok(())
}

async fn tui_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut TuiApp,
    state: &mut AgentState,
) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| render_ui(frame, app, state))?;

        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Global interrupt / quit
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        app.should_quit = true;
                        break;
                    }

                    match app.mode {
                        TuiMode::Insert => match key.code {
                            KeyCode::Esc => {
                                app.mode = TuiMode::Normal;
                                app.status_message =
                                    "-- NORMAL -- [i] Insert | [j/k] Navigate | [q] Quit"
                                        .to_string();
                            }
                            KeyCode::Char('[') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.mode = TuiMode::Normal;
                                app.status_message =
                                    "-- NORMAL -- [i] Insert | [j/k] Navigate | [q] Quit"
                                        .to_string();
                            }
                            KeyCode::Tab => {
                                app.mode = TuiMode::Normal;
                                app.cycle_pane();
                            }
                            KeyCode::BackTab => {
                                app.mode = TuiMode::Normal;
                                app.cycle_pane_reverse();
                            }
                            KeyCode::Enter => {
                                if is_line_continuation(&app.input_text) {
                                    let stripped = strip_line_continuation(&app.input_text);
                                    app.input_text = format!("{}\n", stripped);
                                    app.status_message = "Multi-line input: line continuation added. Press [Enter] without trailing '\\' to submit.".to_string();
                                } else {
                                    submit_input(terminal, app, state).await?;
                                }
                            }
                            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.input_text.push('\n');
                                app.status_message = "Multi-line input: newline added.".to_string();
                            }
                            KeyCode::Backspace => {
                                app.input_text.pop();
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.input_text.clear();
                            }
                            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                let trimmed = app.input_text.trim_end();
                                if let Some(idx) = trimmed.rfind(' ') {
                                    app.input_text.truncate(idx + 1);
                                } else {
                                    app.input_text.clear();
                                }
                            }
                            KeyCode::Char(c) => {
                                app.input_text.push(c);
                            }
                            KeyCode::Up => {
                                if app.auto_scroll {
                                    app.auto_scroll = false;
                                    app.chat_scroll = app.max_chat_scroll.saturating_sub(1);
                                } else {
                                    app.chat_scroll = app.chat_scroll.saturating_sub(1);
                                }
                            }
                            KeyCode::Down => {
                                app.chat_scroll = app.chat_scroll.saturating_add(1);
                                if app.chat_scroll >= app.max_chat_scroll {
                                    app.auto_scroll = true;
                                }
                            }
                            KeyCode::PageUp => {
                                if app.auto_scroll {
                                    app.auto_scroll = false;
                                    app.chat_scroll = app.max_chat_scroll.saturating_sub(10);
                                } else {
                                    app.chat_scroll = app.chat_scroll.saturating_sub(10);
                                }
                            }
                            KeyCode::PageDown => {
                                app.chat_scroll = app.chat_scroll.saturating_add(10);
                                if app.chat_scroll >= app.max_chat_scroll {
                                    app.auto_scroll = true;
                                }
                            }
                            _ => {}
                        },
                        TuiMode::Normal => {
                            let was_g_pending = app.pending_key == Some('g');
                            app.pending_key = None;

                            match key.code {
                                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                                    app.should_quit = true;
                                }
                                KeyCode::Char('i') | KeyCode::Char('a') => {
                                    app.active_pane = ActivePane::Input;
                                    app.mode = TuiMode::Insert;
                                    app.status_message = "-- INSERT -- Type prompt | [Esc] Normal mode | [Enter] Submit".to_string();
                                }
                                KeyCode::Char('o') => {
                                    app.input_text.clear();
                                    app.active_pane = ActivePane::Input;
                                    app.mode = TuiMode::Insert;
                                    app.status_message =
                                        "-- INSERT -- New prompt | [Esc] Normal mode".to_string();
                                }
                                KeyCode::Char('/') => {
                                    app.active_pane = ActivePane::Input;
                                    app.mode = TuiMode::Insert;
                                    app.status_message =
                                        "-- INSERT -- Search / Prompt | [Esc] Normal mode"
                                            .to_string();
                                }
                                KeyCode::Char('x') => {
                                    if app.active_pane == ActivePane::Input {
                                        app.input_text.pop();
                                    }
                                }
                                KeyCode::Tab | KeyCode::Char('w') => {
                                    app.cycle_pane();
                                }
                                KeyCode::BackTab => {
                                    app.cycle_pane_reverse();
                                }
                                KeyCode::Char('b')
                                    if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    app.cycle_pane_reverse();
                                }
                                KeyCode::Char('h') | KeyCode::Left => match app.active_pane {
                                    ActivePane::Chat => app.active_pane = ActivePane::FileTree,
                                    ActivePane::Input => app.active_pane = ActivePane::Subagents,
                                    _ => app.cycle_pane_reverse(),
                                },
                                KeyCode::Char('l') | KeyCode::Right => match app.active_pane {
                                    ActivePane::FileTree => app.active_pane = ActivePane::Chat,
                                    ActivePane::Subagents => app.active_pane = ActivePane::Input,
                                    _ => app.cycle_pane(),
                                },
                                KeyCode::Char('j') | KeyCode::Down => match app.active_pane {
                                    ActivePane::FileTree
                                        if app.selected_file + 1 < app.file_tree.len() =>
                                    {
                                        app.selected_file += 1;
                                    }
                                    ActivePane::Chat | ActivePane::Input => {
                                        app.chat_scroll = app.chat_scroll.saturating_add(1);
                                        if app.chat_scroll >= app.max_chat_scroll {
                                            app.auto_scroll = true;
                                        }
                                    }
                                    _ => {}
                                },
                                KeyCode::Char('k') | KeyCode::Up => match app.active_pane {
                                    ActivePane::FileTree if app.selected_file > 0 => {
                                        app.selected_file -= 1;
                                    }
                                    ActivePane::Chat | ActivePane::Input => {
                                        if app.auto_scroll {
                                            app.auto_scroll = false;
                                            app.chat_scroll = app.max_chat_scroll.saturating_sub(1);
                                        } else {
                                            app.chat_scroll = app.chat_scroll.saturating_sub(1);
                                        }
                                    }
                                    _ => {}
                                },
                                KeyCode::Char('g') => {
                                    if was_g_pending {
                                        if app.active_pane == ActivePane::Chat
                                            || app.active_pane == ActivePane::Input
                                        {
                                            app.auto_scroll = false;
                                            app.chat_scroll = 0;
                                        } else if app.active_pane == ActivePane::FileTree {
                                            app.selected_file = 0;
                                        }
                                        app.status_message = "Jumped to top.".to_string();
                                    } else {
                                        app.pending_key = Some('g');
                                        app.status_message =
                                            "Press 'g' again to jump to top (gg)...".to_string();
                                    }
                                }
                                KeyCode::Char('G') => {
                                    if app.active_pane == ActivePane::Chat
                                        || app.active_pane == ActivePane::Input
                                    {
                                        app.auto_scroll = true;
                                        app.chat_scroll = app.max_chat_scroll;
                                    } else if app.active_pane == ActivePane::FileTree {
                                        app.selected_file = app.file_tree.len().saturating_sub(1);
                                    }
                                    app.status_message = "Jumped to bottom.".to_string();
                                }
                                KeyCode::Char('d')
                                    if key.modifiers.contains(KeyModifiers::CONTROL)
                                        || key.code == KeyCode::PageDown =>
                                {
                                    app.chat_scroll = app.chat_scroll.saturating_add(10);
                                    if app.chat_scroll >= app.max_chat_scroll {
                                        app.auto_scroll = true;
                                    }
                                }
                                KeyCode::Char('u')
                                    if key.modifiers.contains(KeyModifiers::CONTROL)
                                        || key.code == KeyCode::PageUp =>
                                {
                                    if app.auto_scroll {
                                        app.auto_scroll = false;
                                        app.chat_scroll = app.max_chat_scroll.saturating_sub(10);
                                    } else {
                                        app.chat_scroll = app.chat_scroll.saturating_sub(10);
                                    }
                                }
                                KeyCode::Char('f')
                                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    app.chat_scroll = app.chat_scroll.saturating_add(25);
                                    if app.chat_scroll >= app.max_chat_scroll {
                                        app.auto_scroll = true;
                                    }
                                }
                                KeyCode::Char('b')
                                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    if app.auto_scroll {
                                        app.auto_scroll = false;
                                        app.chat_scroll = app.max_chat_scroll.saturating_sub(25);
                                    } else {
                                        app.chat_scroll = app.chat_scroll.saturating_sub(25);
                                    }
                                }
                                KeyCode::Char('v') | KeyCode::Char('V') => {
                                    app.mode = TuiMode::Visual;
                                    match app.active_pane {
                                        ActivePane::Chat => {
                                            app.visual_start = app.chat_scroll as usize;
                                            app.visual_cursor = app.chat_scroll as usize;
                                        }
                                        ActivePane::FileTree => {
                                            app.visual_start = app.selected_file;
                                            app.visual_cursor = app.selected_file;
                                        }
                                        _ => {
                                            app.visual_start = 0;
                                            app.visual_cursor = 0;
                                        }
                                    }
                                    app.status_message =
                                        "-- VISUAL -- [j/k] Select lines | [y] Yank | [Esc] Cancel"
                                            .to_string();
                                }
                                KeyCode::Char('p') | KeyCode::Char('P') => {
                                    if !app.yank_buffer.is_empty() {
                                        app.active_pane = ActivePane::Input;
                                        app.input_text.push_str(&app.yank_buffer);
                                        app.status_message = format!(
                                            "Pasted {} chars into input.",
                                            app.yank_buffer.len()
                                        );
                                    } else {
                                        app.status_message =
                                            "Yank buffer is empty. (Use [v] to select, [y] to yank)"
                                                .to_string();
                                    }
                                }
                                KeyCode::Enter => {
                                    if app.active_pane == ActivePane::FileTree {
                                        if let Some(file) = app.file_tree.get(app.selected_file) {
                                            app.input_text = format!("/file {}", file);
                                            app.active_pane = ActivePane::Input;
                                            app.mode = TuiMode::Insert;
                                            app.status_message =
                                                format!("Attached file: {}. [Enter] submit.", file);
                                        }
                                    } else if app.active_pane == ActivePane::Input {
                                        if is_line_continuation(&app.input_text) {
                                            let stripped = strip_line_continuation(&app.input_text);
                                            app.input_text = format!("{}\n", stripped);
                                            app.status_message = "Multi-line input: line continuation added. Press [Enter] without trailing '\\' to submit.".to_string();
                                        } else {
                                            submit_input(terminal, app, state).await?;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        TuiMode::Visual => match key.code {
                            KeyCode::Esc
                            | KeyCode::Char('q')
                            | KeyCode::Char('v')
                            | KeyCode::Char('V') => {
                                app.mode = TuiMode::Normal;
                                app.status_message =
                                    "-- NORMAL -- Visual selection canceled.".to_string();
                            }
                            KeyCode::Tab => {
                                app.mode = TuiMode::Normal;
                                app.cycle_pane();
                            }
                            KeyCode::BackTab => {
                                app.mode = TuiMode::Normal;
                                app.cycle_pane_reverse();
                            }
                            KeyCode::Char('j') | KeyCode::Down => match app.active_pane {
                                ActivePane::Chat => {
                                    let chat_lines = build_chat_lines(state, &app.streaming_reply);
                                    if app.visual_cursor + 1 < chat_lines.len() {
                                        app.visual_cursor += 1;
                                        if app.visual_cursor as u16
                                            > app.chat_scroll.saturating_add(15)
                                        {
                                            app.chat_scroll = app.chat_scroll.saturating_add(1);
                                        }
                                    }
                                }
                                ActivePane::FileTree
                                    if app.visual_cursor + 1 < app.file_tree.len() =>
                                {
                                    app.visual_cursor += 1;
                                    app.selected_file = app.visual_cursor;
                                }
                                _ => {}
                            },
                            KeyCode::Char('k') | KeyCode::Up => match app.active_pane {
                                ActivePane::Chat => {
                                    if app.visual_cursor > 0 {
                                        app.visual_cursor -= 1;
                                        if (app.visual_cursor as u16) < app.chat_scroll {
                                            app.chat_scroll = app.visual_cursor as u16;
                                            app.auto_scroll = false;
                                        }
                                    }
                                }
                                ActivePane::FileTree if app.visual_cursor > 0 => {
                                    app.visual_cursor -= 1;
                                    app.selected_file = app.visual_cursor;
                                }
                                _ => {}
                            },
                            KeyCode::Char('g') => {
                                app.visual_cursor = 0;
                                if app.active_pane == ActivePane::Chat {
                                    app.chat_scroll = 0;
                                    app.auto_scroll = false;
                                } else if app.active_pane == ActivePane::FileTree {
                                    app.selected_file = 0;
                                }
                            }
                            KeyCode::Char('G') => {
                                if app.active_pane == ActivePane::Chat {
                                    let chat_lines = build_chat_lines(state, &app.streaming_reply);
                                    app.visual_cursor = chat_lines.len().saturating_sub(1);
                                    app.chat_scroll = app.max_chat_scroll;
                                } else if app.active_pane == ActivePane::FileTree {
                                    app.visual_cursor = app.file_tree.len().saturating_sub(1);
                                    app.selected_file = app.visual_cursor;
                                }
                            }
                            KeyCode::Char('y') | KeyCode::Char('Y') => match app.active_pane {
                                ActivePane::Chat => {
                                    let chat_lines = build_chat_lines(state, &app.streaming_reply);
                                    let sel_min = app.visual_start.min(app.visual_cursor);
                                    let sel_max = app
                                        .visual_start
                                        .max(app.visual_cursor)
                                        .min(chat_lines.len().saturating_sub(1));
                                    if sel_min <= sel_max && !chat_lines.is_empty() {
                                        let selected_text: Vec<String> = chat_lines
                                            [sel_min..=sel_max]
                                            .iter()
                                            .map(|line| {
                                                line.spans
                                                    .iter()
                                                    .map(|s| s.content.as_ref())
                                                    .collect::<Vec<&str>>()
                                                    .join("")
                                            })
                                            .collect();
                                        let count = selected_text.len();
                                        app.yank_buffer = selected_text.join("\n");
                                        app.mode = TuiMode::Normal;
                                        app.status_message = format!(
                                                "-- NORMAL -- Yanked {} lines. Press [p] to paste into input.",
                                                count
                                            );
                                    } else {
                                        app.mode = TuiMode::Normal;
                                        app.status_message =
                                            "-- NORMAL -- No lines selected to yank.".to_string();
                                    }
                                }
                                ActivePane::FileTree => {
                                    let sel_min = app.visual_start.min(app.visual_cursor);
                                    let sel_max = app
                                        .visual_start
                                        .max(app.visual_cursor)
                                        .min(app.file_tree.len().saturating_sub(1));
                                    if sel_min <= sel_max && !app.file_tree.is_empty() {
                                        let files = &app.file_tree[sel_min..=sel_max];
                                        let count = files.len();
                                        app.yank_buffer = files.join("\n");
                                        app.mode = TuiMode::Normal;
                                        app.status_message = format!(
                                                "-- NORMAL -- Yanked {} file paths. Press [p] to paste into input.",
                                                count
                                            );
                                    } else {
                                        app.mode = TuiMode::Normal;
                                    }
                                }
                                _ => {
                                    app.mode = TuiMode::Normal;
                                }
                            },
                            _ => {}
                        },
                    }
                }
            }
        }
    }

    Ok(())
}

fn render_ui(frame: &mut ratatui::Frame, app: &mut TuiApp, state: &AgentState) {
    let size = frame.area();

    // Main layout: Header, Content, Footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(10),   // Content
            Constraint::Length(1), // Footer status bar
        ])
        .split(size);

    render_header(frame, chunks[0], state);
    render_main_content(frame, chunks[1], app, state);
    render_footer(frame, chunks[2], app);
}

fn render_header(frame: &mut ratatui::Frame, area: Rect, state: &AgentState) {
    let header_text = vec![Line::from(vec![
        Span::styled(
            "🛰️  ORBIT TUI v0.2 ",
            Style::default().fg(Color::Cyan).bold(),
        ),
        Span::raw(" | Model: "),
        Span::styled(
            &state.config.active_model,
            Style::default().fg(Color::Green).bold(),
        ),
        Span::raw(" | MCP Servers: "),
        Span::styled(
            format!("{}", state.mcp_clients.len()),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" | "),
        Span::styled(
            if state.project_rules.is_some() {
                "Rules: Loaded"
            } else {
                "Rules: None"
            },
            Style::default().fg(Color::Magenta),
        ),
        Span::raw(" | Vim: "),
        Span::styled(
            if state.config.vim_mode { "ON" } else { "OFF" },
            Style::default().fg(Color::Cyan).bold(),
        ),
    ])];

    let header = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(header, area);
}

fn render_main_content(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &mut TuiApp,
    state: &AgentState,
) {
    // Horizontal Split: Left Sidebar (30%), Right Workspace (70%)
    let horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
        .split(area);

    // Left Sidebar: File Tree (60%), Subagents (40%)
    let sidebar_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(horizontal_chunks[0]);

    render_file_tree(frame, sidebar_chunks[0], app);
    render_subagents(frame, sidebar_chunks[1], app, state);

    // Right Workspace: Chat Display, Input Box (dynamic height for multi-line)
    let input_lines_count = app.input_text.split('\n').count() as u16;
    let input_height = (input_lines_count + 2).clamp(3, 8);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(input_height)])
        .split(horizontal_chunks[1]);

    render_chat_pane(frame, right_chunks[0], app, state);
    render_input_pane(frame, right_chunks[1], app);
}

fn render_file_tree(frame: &mut ratatui::Frame, area: Rect, app: &TuiApp) {
    let border_color = if app.active_pane == ActivePane::FileTree {
        if app.mode == TuiMode::Visual {
            Color::Magenta
        } else {
            Color::Yellow
        }
    } else {
        Color::Blue
    };

    let sel_min = app.visual_start.min(app.visual_cursor);
    let sel_max = app.visual_start.max(app.visual_cursor);

    let items: Vec<ListItem> = app
        .file_tree
        .iter()
        .enumerate()
        .map(|(idx, file)| {
            let style = if app.active_pane == ActivePane::FileTree
                && app.mode == TuiMode::Visual
                && idx >= sel_min
                && idx <= sel_max
            {
                Style::default()
                    .fg(Color::Magenta)
                    .bold()
                    .add_modifier(Modifier::REVERSED)
            } else if idx == app.selected_file && app.active_pane == ActivePane::FileTree {
                Style::default()
                    .fg(Color::Yellow)
                    .bold()
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(format!("📄 {}", file)).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(Span::styled(
                " 📁 Workspace Files ",
                Style::default().fg(Color::White).bold(),
            ))
            .border_style(Style::default().fg(border_color)),
    );

    frame.render_widget(list, area);
}

fn render_subagents(frame: &mut ratatui::Frame, area: Rect, app: &TuiApp, state: &AgentState) {
    let border_color = if app.active_pane == ActivePane::Subagents {
        Color::Yellow
    } else {
        Color::Blue
    };

    let tasks = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(state.subagent_mgr.list_subagents())
    });

    let items: Vec<ListItem> = if tasks.is_empty() {
        vec![ListItem::new("No active subagent tasks.").style(Style::default().fg(Color::Cyan))]
    } else {
        tasks
            .into_iter()
            .map(|t| {
                let status_style = match &t.status {
                    SubagentStatus::Running => Style::default().fg(Color::Yellow),
                    SubagentStatus::Completed => Style::default().fg(Color::Green),
                    SubagentStatus::Failed(_) => Style::default().fg(Color::Red),
                };
                let status_label = match &t.status {
                    SubagentStatus::Running => "RUNNING",
                    SubagentStatus::Completed => "DONE",
                    SubagentStatus::Failed(_) => "FAIL",
                };
                ListItem::new(vec![Line::from(vec![
                    Span::styled(format!("#{:<2} ", t.id), Style::default().bold()),
                    Span::styled(t.role, Style::default().cyan()),
                    Span::raw(" ["),
                    Span::styled(status_label, status_style.bold()),
                    Span::raw("]"),
                ])])
            })
            .collect()
    };

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(Span::styled(
                " 🤖 Subagents ",
                Style::default().fg(Color::White).bold(),
            ))
            .border_style(Style::default().fg(border_color)),
    );

    frame.render_widget(list, area);
}

fn format_message_lines(lines: &mut Vec<Line>, content: &str, default_style: Style) {
    let mut in_code_fence = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if line.starts_with("```tool:") || line.starts_with("[TOOL OUTPUT") {
            lines.push(Line::from(Span::styled(
                format!("  {}", line),
                Style::default().fg(Color::Yellow).bold(),
            )));
        } else if line.starts_with("```") {
            in_code_fence = !in_code_fence;
            lines.push(Line::from(Span::styled(
                format!("  {}", line),
                Style::default().fg(Color::Cyan).bold(),
            )));
        } else if in_code_fence {
            let code_style = if trimmed.starts_with("//") || trimmed.starts_with('#') {
                Style::default().fg(Color::Green)
            } else if trimmed.starts_with("fn ")
                || trimmed.starts_with("pub ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("def ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("import ")
            {
                Style::default().fg(Color::LightMagenta).bold()
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::from(Span::styled(format!("  {}", line), code_style)));
        } else if trimmed.starts_with("===")
            || (trimmed.starts_with('[') && trimmed.ends_with("):"))
        {
            lines.push(Line::from(Span::styled(
                format!("  {}", line),
                Style::default().fg(Color::Cyan).bold(),
            )));
        } else if let Some((left, right)) = trimmed.split_once(" — ") {
            if left.starts_with("• ") {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("    {}", left),
                        Style::default().fg(Color::LightGreen).bold(),
                    ),
                    Span::styled(format!(" — {}", right), Style::default().fg(Color::White)),
                ]));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("  {}", line),
                    default_style,
                )));
            }
        } else {
            lines.push(Line::from(Span::styled(
                format!("  {}", line),
                default_style,
            )));
        }
    }
}

pub fn build_chat_lines(state: &AgentState, streaming_reply: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if state.history.is_empty() && streaming_reply.is_empty() {
        lines.push(Line::from(Span::styled(
            "Welcome to Orbit TUI! Type your prompt below and press [Enter].",
            Style::default().fg(Color::Cyan).bold(),
        )));
    } else {
        for msg in &state.history {
            let (role_span, content_style) = match msg.role.as_str() {
                "user" => (
                    Span::styled("🧑 User: ", Style::default().fg(Color::Magenta).bold()),
                    Style::default().fg(Color::White).bold(),
                ),
                "assistant" => (
                    Span::styled("🤖 Orbit: ", Style::default().fg(Color::Green).bold()),
                    Style::default().fg(Color::White),
                ),
                "system" => (
                    Span::styled("⚙️ System: ", Style::default().fg(Color::Cyan).bold()),
                    Style::default().fg(Color::White),
                ),
                _ => (
                    Span::styled(format!("{}: ", msg.role), Style::default().bold()),
                    Style::default().fg(Color::White),
                ),
            };

            lines.push(Line::from(role_span));
            format_message_lines(&mut lines, &msg.content, content_style);
            lines.push(Line::from(""));
        }

        if !streaming_reply.is_empty() {
            lines.push(Line::from(Span::styled(
                "🤖 Orbit: ",
                Style::default().fg(Color::Green).bold(),
            )));
            format_message_lines(
                &mut lines,
                streaming_reply,
                Style::default().fg(Color::White),
            );
            lines.push(Line::from(""));
        }
    }
    lines
}

fn render_chat_pane(frame: &mut ratatui::Frame, area: Rect, app: &mut TuiApp, state: &AgentState) {
    let border_color = if app.active_pane == ActivePane::Chat {
        if app.mode == TuiMode::Visual {
            Color::Magenta
        } else {
            Color::Yellow
        }
    } else {
        Color::Blue
    };

    let mut lines = build_chat_lines(state, &app.streaming_reply);

    let total_lines = lines.len();
    let inner_height = area.height.saturating_sub(2) as usize;
    let max_scroll = total_lines.saturating_sub(inner_height) as u16;
    app.max_chat_scroll = max_scroll;

    if app.mode == TuiMode::Visual && app.active_pane == ActivePane::Chat {
        let sel_min = app.visual_start.min(app.visual_cursor);
        let sel_max = app.visual_start.max(app.visual_cursor);
        for (idx, line) in lines.iter_mut().enumerate() {
            if idx >= sel_min && idx <= sel_max {
                *line = line.clone().patch_style(
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::REVERSED),
                );
            }
        }
    }

    let scroll_pos = if app.auto_scroll {
        max_scroll
    } else {
        app.chat_scroll.min(max_scroll)
    };

    let title_text = if app.mode == TuiMode::Visual && app.active_pane == ActivePane::Chat {
        " 💬 Conversation & Output [VISUAL SELECTION] "
    } else if app.auto_scroll {
        " 💬 Conversation & Output [Auto-Scroll] "
    } else {
        " 💬 Conversation & Output [Scrolled] "
    };

    let paragraph = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(
                    title_text,
                    Style::default().fg(Color::White).bold(),
                ))
                .border_style(Style::default().fg(border_color)),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll_pos, 0));

    frame.render_widget(paragraph, area);
}

fn render_input_pane(frame: &mut ratatui::Frame, area: Rect, app: &TuiApp) {
    let (border_color, mode_badge) = match (app.active_pane, app.mode) {
        (ActivePane::Input, TuiMode::Insert) => (
            Color::Green,
            Span::styled(" [INSERT] ", Style::default().fg(Color::Green).bold()),
        ),
        (ActivePane::Input, TuiMode::Normal) => (
            Color::Yellow,
            Span::styled(" [NORMAL] ", Style::default().fg(Color::Yellow).bold()),
        ),
        (ActivePane::Input, TuiMode::Visual) => (
            Color::Magenta,
            Span::styled(" [VISUAL] ", Style::default().fg(Color::Magenta).bold()),
        ),
        _ => (
            Color::Blue,
            Span::styled(
                match app.mode {
                    TuiMode::Visual => " [VISUAL] ",
                    TuiMode::Normal => " [NORMAL] ",
                    TuiMode::Insert => " [INACTIVE] ",
                },
                match app.mode {
                    TuiMode::Visual => Style::default().fg(Color::Magenta).bold(),
                    TuiMode::Normal => Style::default().fg(Color::Yellow),
                    TuiMode::Insert => Style::default().fg(Color::Cyan),
                },
            ),
        ),
    };

    let cursor_char = match app.mode {
        TuiMode::Insert => "█",
        TuiMode::Normal => "▒",
        TuiMode::Visual => "▚",
    };

    let title_line = Line::from(vec![
        Span::styled(" ⌨️ Input Prompt", Style::default().fg(Color::White).bold()),
        mode_badge,
        Span::styled(
            match app.mode {
                TuiMode::Insert => " [Enter: Send | Esc: Normal] ",
                TuiMode::Normal => " [i: Insert | v: Visual | j/k: Scroll | p: Paste | Tab: Pane] ",
                TuiMode::Visual => " [j/k: Select lines | y: Yank | Esc: Cancel] ",
            },
            Style::default().fg(Color::Cyan),
        ),
    ]);

    let lines: Vec<Line> = if app.input_text.contains('\n') {
        let raw_lines: Vec<&str> = app.input_text.split('\n').collect();
        let total = raw_lines.len();
        raw_lines
            .into_iter()
            .enumerate()
            .map(|(i, l)| {
                let prefix = if i == 0 { "> " } else { "... " };
                let suffix = if i == total - 1 { cursor_char } else { "" };
                Line::from(vec![
                    Span::styled(prefix, Style::default().fg(Color::Cyan).bold()),
                    Span::raw(format!("{}{}", l, suffix)),
                ])
            })
            .collect()
    } else {
        vec![Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan).bold()),
            Span::raw(format!("{}{}", app.input_text, cursor_char)),
        ])]
    };

    let inner_height = area.height.saturating_sub(2) as usize;
    let scroll_y = if lines.len() > inner_height {
        (lines.len() - inner_height) as u16
    } else {
        0
    };

    let input_p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(title_line)
                .border_style(Style::default().fg(border_color)),
        )
        .scroll((scroll_y, 0));

    frame.render_widget(input_p, area);
}

fn render_footer(frame: &mut ratatui::Frame, area: Rect, app: &TuiApp) {
    let (mode_badge, mode_hint) = match app.mode {
        TuiMode::Normal => (
            Span::styled(
                " NORMAL ",
                Style::default().bg(Color::Yellow).fg(Color::Black).bold(),
            ),
            " | [j/k] move/scroll | [h/l] panes | [v] visual | [p] paste | [i] insert | [q] quit",
        ),
        TuiMode::Insert => (
            Span::styled(
                " INSERT ",
                Style::default().bg(Color::Green).fg(Color::Black).bold(),
            ),
            " | [Enter] submit prompt | [Esc] normal mode | [Tab] switch pane",
        ),
        TuiMode::Visual => (
            Span::styled(
                " VISUAL ",
                Style::default().bg(Color::Magenta).fg(Color::Black).bold(),
            ),
            " | [j/k] expand selection | [y] yank | [Esc] cancel",
        ),
    };

    let footer = Paragraph::new(Line::from(vec![
        mode_badge,
        Span::raw(" "),
        Span::styled(&app.status_message, Style::default().fg(Color::Cyan)),
        Span::raw(mode_hint),
        Span::raw(" | Pane: "),
        Span::styled(
            match app.active_pane {
                ActivePane::Input => "Input",
                ActivePane::Chat => "Chat",
                ActivePane::FileTree => "Files",
                ActivePane::Subagents => "Subagents",
            },
            Style::default().fg(Color::Yellow).bold(),
        ),
    ]));
    frame.render_widget(footer, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tui_app_initialization() {
        let temp_dir = std::env::temp_dir().join("orbit_test_tui_app");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("main.rs"), "fn main() {}").unwrap();

        let app = TuiApp::new(&temp_dir);
        assert_eq!(app.active_pane, ActivePane::Input);
        assert!(!app.file_tree.is_empty());
        assert_eq!(app.file_tree[0], "main.rs");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_tui_app_pane_cycling() {
        let temp_dir = std::env::temp_dir().join("orbit_test_tui_panes");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let mut app = TuiApp::new(&temp_dir);
        assert_eq!(app.active_pane, ActivePane::Input);
        app.cycle_pane();
        assert_eq!(app.active_pane, ActivePane::FileTree);
        app.cycle_pane();
        assert_eq!(app.active_pane, ActivePane::Subagents);
        app.cycle_pane();
        assert_eq!(app.active_pane, ActivePane::Chat);
        app.cycle_pane();
        assert_eq!(app.active_pane, ActivePane::Input);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_tui_app_vim_mode_and_reverse_cycle() {
        let temp_dir = std::env::temp_dir().join("orbit_test_tui_vim");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let mut app = TuiApp::new(&temp_dir);
        assert_eq!(app.active_pane, ActivePane::Input);
        assert_eq!(app.mode, TuiMode::Insert);

        // Cycling pane away from Input transitions mode to Normal
        app.cycle_pane();
        assert_eq!(app.active_pane, ActivePane::FileTree);
        assert_eq!(app.mode, TuiMode::Normal);

        // Reverse cycling works
        app.cycle_pane_reverse();
        assert_eq!(app.active_pane, ActivePane::Input);

        // Setting insert mode manually
        app.mode = TuiMode::Insert;
        assert_eq!(app.mode, TuiMode::Insert);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_tui_app_visual_mode_selection_and_yank_paste() {
        let temp_dir = std::env::temp_dir().join("orbit_test_tui_visual");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("file_a.rs"), "fn a() {}").unwrap();
        std::fs::write(temp_dir.join("file_b.rs"), "fn b() {}").unwrap();

        let mut app = TuiApp::new(&temp_dir);
        assert_eq!(app.mode, TuiMode::Insert);

        // Switch to Normal mode
        app.mode = TuiMode::Normal;
        assert_eq!(app.mode, TuiMode::Normal);

        // Switch to Visual mode
        app.mode = TuiMode::Visual;
        app.visual_start = 0;
        app.visual_cursor = 1;
        assert_eq!(app.mode, TuiMode::Visual);

        // Yank simulated content
        app.yank_buffer = "fn a() {}\nfn b() {}".to_string();
        app.mode = TuiMode::Normal;

        // Paste into input_text
        app.active_pane = ActivePane::Input;
        app.input_text.push_str(&app.yank_buffer);
        assert_eq!(app.input_text, "fn a() {}\nfn b() {}");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
