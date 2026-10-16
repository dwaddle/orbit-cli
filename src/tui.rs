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

use crate::outline::{is_home_or_root, is_ignored_path};
use crate::subagent::SubagentStatus;
use crate::AgentState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
    Input,
    FileTree,
    Subagents,
    Chat,
}

pub struct TuiApp {
    pub active_pane: ActivePane,
    pub input_text: String,
    pub file_tree: Vec<String>,
    pub selected_file: usize,
    pub chat_scroll: u16,
    pub max_chat_scroll: u16,
    pub auto_scroll: bool,
    pub streaming_reply: String,
    pub should_quit: bool,
    pub status_message: String,
}

impl TuiApp {
    pub fn new(workspace_dir: &Path) -> Self {
        let mut file_tree = Vec::new();
        collect_file_tree(workspace_dir, workspace_dir, 0, &mut file_tree);
        file_tree.sort();

        Self {
            active_pane: ActivePane::Input,
            input_text: String::new(),
            file_tree,
            selected_file: 0,
            chat_scroll: 0,
            max_chat_scroll: 0,
            auto_scroll: true,
            streaming_reply: String::new(),
            should_quit: false,
            status_message: "Press [Tab] to switch panes, [Enter] to submit, [Esc] to exit."
                .to_string(),
        }
    }

    pub fn cycle_pane(&mut self) {
        self.active_pane = match self.active_pane {
            ActivePane::Input => ActivePane::FileTree,
            ActivePane::FileTree => ActivePane::Subagents,
            ActivePane::Subagents => ActivePane::Chat,
            ActivePane::Chat => ActivePane::Input,
        };
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
                    match key.code {
                        KeyCode::Esc => {
                            app.should_quit = true;
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.should_quit = true;
                        }
                        KeyCode::Tab => {
                            app.cycle_pane();
                        }
                        KeyCode::Enter if app.active_pane == ActivePane::Input => {
                            let input = app.input_text.trim().to_string();
                            if !input.is_empty() {
                                app.input_text.clear();
                                state.history.push(crate::Message {
                                    role: "user".to_string(),
                                    content: input.clone(),
                                });

                                // Run auto-pruning
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
                                app.status_message = format!(
                                    "Generating response with {}...",
                                    state.config.active_model
                                );
                                terminal.draw(|frame| render_ui(frame, app, state))?;

                                let full_result = {
                                    let (tx, mut rx) =
                                        tokio::sync::mpsc::unbounded_channel::<String>();
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
                                            content: full_reply,
                                        });
                                        app.streaming_reply.clear();
                                        app.status_message = "Ready. [Tab] switch panes | [Up/Down] scroll | [Esc] exit".to_string();
                                    }
                                    Some(Err(e)) => {
                                        app.status_message = format!("Error: {}", e);
                                        if !app.streaming_reply.is_empty() {
                                            state.history.push(crate::Message {
                                                role: "assistant".to_string(),
                                                content: std::mem::take(&mut app.streaming_reply),
                                            });
                                        }
                                    }
                                    None => {
                                        if !app.streaming_reply.is_empty() {
                                            state.history.push(crate::Message {
                                                role: "assistant".to_string(),
                                                content: std::mem::take(&mut app.streaming_reply),
                                            });
                                        }
                                    }
                                }
                                terminal.draw(|frame| render_ui(frame, app, state))?;
                            }
                        }
                        KeyCode::Char(c) if app.active_pane == ActivePane::Input => {
                            app.input_text.push(c);
                        }
                        KeyCode::Backspace if app.active_pane == ActivePane::Input => {
                            app.input_text.pop();
                        }
                        KeyCode::Up => match app.active_pane {
                            ActivePane::FileTree if app.selected_file > 0 => {
                                app.selected_file -= 1;
                            }
                            ActivePane::Chat => {
                                if app.auto_scroll {
                                    app.auto_scroll = false;
                                    app.chat_scroll = app.max_chat_scroll.saturating_sub(1);
                                } else {
                                    app.chat_scroll = app.chat_scroll.saturating_sub(1);
                                }
                            }
                            _ => {}
                        },
                        KeyCode::Down => match app.active_pane {
                            ActivePane::FileTree if app.selected_file + 1 < app.file_tree.len() => {
                                app.selected_file += 1;
                            }
                            ActivePane::Chat => {
                                app.chat_scroll = app.chat_scroll.saturating_add(1);
                                if app.chat_scroll >= app.max_chat_scroll {
                                    app.auto_scroll = true;
                                }
                            }
                            _ => {}
                        },
                        KeyCode::PageUp if app.active_pane == ActivePane::Chat => {
                            if app.auto_scroll {
                                app.auto_scroll = false;
                                app.chat_scroll = app.max_chat_scroll.saturating_sub(10);
                            } else {
                                app.chat_scroll = app.chat_scroll.saturating_sub(10);
                            }
                        }
                        KeyCode::PageDown if app.active_pane == ActivePane::Chat => {
                            app.chat_scroll = app.chat_scroll.saturating_add(10);
                            if app.chat_scroll >= app.max_chat_scroll {
                                app.auto_scroll = true;
                            }
                        }
                        _ => {}
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

    // Right Workspace: Chat Display (85%), Input Box (15%)
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(3)])
        .split(horizontal_chunks[1]);

    render_chat_pane(frame, right_chunks[0], app, state);
    render_input_pane(frame, right_chunks[1], app);
}

fn render_file_tree(frame: &mut ratatui::Frame, area: Rect, app: &TuiApp) {
    let border_color = if app.active_pane == ActivePane::FileTree {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let items: Vec<ListItem> = app
        .file_tree
        .iter()
        .enumerate()
        .map(|(idx, file)| {
            let style = if idx == app.selected_file && app.active_pane == ActivePane::FileTree {
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
                Style::default().bold(),
            ))
            .border_style(Style::default().fg(border_color)),
    );

    frame.render_widget(list, area);
}

fn render_subagents(frame: &mut ratatui::Frame, area: Rect, app: &TuiApp, state: &AgentState) {
    let border_color = if app.active_pane == ActivePane::Subagents {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let tasks = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(state.subagent_mgr.list_subagents())
    });

    let items: Vec<ListItem> = if tasks.is_empty() {
        vec![ListItem::new("No active subagent tasks.").style(Style::default().fg(Color::DarkGray))]
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
            .title(Span::styled(" 🤖 Subagents ", Style::default().bold()))
            .border_style(Style::default().fg(border_color)),
    );

    frame.render_widget(list, area);
}

fn format_message_lines(lines: &mut Vec<Line>, content: &str, default_style: Style) {
    let mut in_code_fence = false;
    for line in content.lines() {
        if line.starts_with("```tool:") || line.starts_with("[TOOL OUTPUT") {
            lines.push(Line::from(Span::styled(
                format!("  {}", line),
                Style::default().fg(Color::Yellow),
            )));
        } else if line.starts_with("```") {
            in_code_fence = !in_code_fence;
            lines.push(Line::from(Span::styled(
                format!("  {}", line),
                Style::default().fg(Color::Cyan).bold(),
            )));
        } else if in_code_fence {
            let trimmed = line.trim_start();
            let code_style = if trimmed.starts_with("//") || trimmed.starts_with('#') {
                Style::default().fg(Color::DarkGray)
            } else if trimmed.starts_with("fn ")
                || trimmed.starts_with("pub ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("def ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("import ")
            {
                Style::default().fg(Color::LightMagenta)
            } else {
                Style::default().fg(Color::LightCyan)
            };
            lines.push(Line::from(Span::styled(format!("  {}", line), code_style)));
        } else {
            lines.push(Line::from(Span::styled(
                format!("  {}", line),
                default_style,
            )));
        }
    }
}

fn render_chat_pane(frame: &mut ratatui::Frame, area: Rect, app: &mut TuiApp, state: &AgentState) {
    let border_color = if app.active_pane == ActivePane::Chat {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let mut lines = Vec::new();
    if state.history.is_empty() && app.streaming_reply.is_empty() {
        lines.push(Line::from(Span::styled(
            "Welcome to Orbit TUI! Type your prompt below and press [Enter].",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for msg in &state.history {
            let (role_span, content_style) = match msg.role.as_str() {
                "user" => (
                    Span::styled("🧑 User: ", Style::default().fg(Color::Magenta).bold()),
                    Style::default().fg(Color::White),
                ),
                "assistant" => (
                    Span::styled("🤖 Orbit: ", Style::default().fg(Color::Green).bold()),
                    Style::default().fg(Color::Gray),
                ),
                "system" => (
                    Span::styled("⚙️ System: ", Style::default().fg(Color::Cyan).bold()),
                    Style::default().fg(Color::DarkGray),
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

        if !app.streaming_reply.is_empty() {
            lines.push(Line::from(Span::styled(
                "🤖 Orbit: ",
                Style::default().fg(Color::Green).bold(),
            )));
            format_message_lines(
                &mut lines,
                &app.streaming_reply,
                Style::default().fg(Color::Gray),
            );
            lines.push(Line::from(""));
        }
    }

    let total_lines = lines.len();
    let inner_height = area.height.saturating_sub(2) as usize;
    let max_scroll = total_lines.saturating_sub(inner_height) as u16;
    app.max_chat_scroll = max_scroll;

    let scroll_pos = if app.auto_scroll {
        max_scroll
    } else {
        app.chat_scroll.min(max_scroll)
    };

    let title_text = if app.auto_scroll {
        " 💬 Conversation & Output [Auto-Scroll] "
    } else {
        " 💬 Conversation & Output [Scrolled] "
    };

    let paragraph = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(title_text, Style::default().bold()))
                .border_style(Style::default().fg(border_color)),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll_pos, 0));

    frame.render_widget(paragraph, area);
}

fn render_input_pane(frame: &mut ratatui::Frame, area: Rect, app: &TuiApp) {
    let border_color = if app.active_pane == ActivePane::Input {
        Color::Green
    } else {
        Color::DarkGray
    };

    let input_p = Paragraph::new(format!("> {}█", app.input_text)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(Span::styled(
                " ⌨️ Input Prompt [Enter: Submit] ",
                Style::default().bold(),
            ))
            .border_style(Style::default().fg(border_color)),
    );

    frame.render_widget(input_p, area);
}

fn render_footer(frame: &mut ratatui::Frame, area: Rect, app: &TuiApp) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" Status: ", Style::default().bold()),
        Span::styled(&app.status_message, Style::default().fg(Color::Cyan)),
        Span::raw(" | "),
        Span::styled(
            format!("Active Pane: {:?}", app.active_pane),
            Style::default().fg(Color::Yellow),
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
}
