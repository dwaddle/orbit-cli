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

use crate::outline::is_ignored_path;
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
    pub should_quit: bool,
    pub status_message: String,
}

impl TuiApp {
    pub fn new(workspace_dir: &Path) -> Self {
        let mut file_tree = Vec::new();
        collect_file_tree(workspace_dir, workspace_dir, &mut file_tree);
        file_tree.sort();

        Self {
            active_pane: ActivePane::Input,
            input_text: String::new(),
            file_tree,
            selected_file: 0,
            chat_scroll: 0,
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

fn collect_file_tree(current_dir: &Path, base_dir: &Path, out: &mut Vec<String>) {
    let entries = match std::fs::read_dir(current_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if is_ignored_path(&path) {
            continue;
        }

        if path.is_dir() {
            collect_file_tree(&path, base_dir, out);
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
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiApp::new(&state.workspace_dir);

    let res = tui_event_loop(&mut terminal, &mut app, state).await;

    // Restore terminal safely
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
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

                                app.status_message =
                                    format!("Generating response for `{}`...", input);
                                terminal.draw(|frame| render_ui(frame, app, state))?;

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

                                match crate::stream_completion(state).await {
                                    Ok(reply) => {
                                        state.history.push(crate::Message {
                                            role: "assistant".to_string(),
                                            content: reply,
                                        });
                                        app.status_message = "Ready.".to_string();
                                    }
                                    Err(e) => {
                                        app.status_message = format!("Error: {}", e);
                                    }
                                }
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
                            ActivePane::Chat if app.chat_scroll > 0 => {
                                app.chat_scroll = app.chat_scroll.saturating_sub(1);
                            }
                            _ => {}
                        },
                        KeyCode::Down => match app.active_pane {
                            ActivePane::FileTree if app.selected_file + 1 < app.file_tree.len() => {
                                app.selected_file += 1;
                            }
                            ActivePane::Chat => {
                                app.chat_scroll = app.chat_scroll.saturating_add(1);
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}

fn render_ui(frame: &mut ratatui::Frame, app: &TuiApp, state: &AgentState) {
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

fn render_main_content(frame: &mut ratatui::Frame, area: Rect, app: &TuiApp, state: &AgentState) {
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

fn render_chat_pane(frame: &mut ratatui::Frame, area: Rect, app: &TuiApp, state: &AgentState) {
    let border_color = if app.active_pane == ActivePane::Chat {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let mut lines = Vec::new();
    if state.history.is_empty() {
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
            for line in msg.content.lines() {
                if line.starts_with("```tool:") || line.starts_with("[TOOL OUTPUT") {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", line),
                        Style::default().fg(Color::Yellow),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", line),
                        content_style,
                    )));
                }
            }
            lines.push(Line::from(""));
        }
    }

    let paragraph = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(
                    " 💬 Conversation & Output ",
                    Style::default().bold(),
                ))
                .border_style(Style::default().fg(border_color)),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.chat_scroll, 0));

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
