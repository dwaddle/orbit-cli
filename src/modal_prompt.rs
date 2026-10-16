use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use rustyline::error::ReadlineError;
use std::io::{self, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalMode {
    Normal,
    Insert,
    Visual,
}

pub struct RawModeGuard;

impl RawModeGuard {
    pub fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(RawModeGuard)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = write!(io::stdout(), "\x1b[2 q"); // Reset cursor to steady block
        let _ = io::stdout().flush();
    }
}

pub struct ModalPromptState {
    pub mode: ModalMode,
    pub buffer: String,
    pub cursor: usize,                // Byte offset in buffer
    pub visual_anchor: Option<usize>, // Byte offset for visual selection start
    pub yank_buffer: String,
    pub pending_key: Option<char>,
    pub undo_stack: Vec<(String, usize)>,
}

impl ModalPromptState {
    pub fn new() -> Self {
        Self {
            mode: ModalMode::Insert,
            buffer: String::new(),
            cursor: 0,
            visual_anchor: None,
            yank_buffer: String::new(),
            pending_key: None,
            undo_stack: Vec::new(),
        }
    }

    pub fn save_undo(&mut self) {
        if self.undo_stack.len() >= 50 {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push((self.buffer.clone(), self.cursor));
    }

    pub fn undo(&mut self) -> bool {
        if let Some((prev_buf, prev_cur)) = self.undo_stack.pop() {
            self.buffer = prev_buf;
            self.cursor = prev_cur.min(self.buffer.len());
            true
        } else {
            false
        }
    }
}

impl Default for ModalPromptState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn prev_char_boundary(s: &str, byte_idx: usize) -> usize {
    if byte_idx == 0 {
        return 0;
    }
    let mut idx = byte_idx - 1;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

pub fn next_char_boundary(s: &str, byte_idx: usize) -> usize {
    if byte_idx >= s.len() {
        return s.len();
    }
    let mut idx = byte_idx + 1;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

pub fn prev_word_boundary(s: &str, byte_idx: usize) -> usize {
    if byte_idx == 0 {
        return 0;
    }
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let current_char_pos = chars
        .iter()
        .position(|(idx, _)| *idx >= byte_idx)
        .unwrap_or(chars.len());
    if current_char_pos == 0 {
        return 0;
    }
    let mut i = current_char_pos - 1;
    while i > 0 && chars[i].1.is_whitespace() {
        i -= 1;
    }
    if chars[i].1.is_alphanumeric() || chars[i].1 == '_' {
        while i > 0 && (chars[i - 1].1.is_alphanumeric() || chars[i - 1].1 == '_') {
            i -= 1;
        }
    } else if !chars[i].1.is_whitespace() {
        while i > 0 && !chars[i - 1].1.is_alphanumeric() && !chars[i - 1].1.is_whitespace() {
            i -= 1;
        }
    }
    chars[i].0
}

pub fn next_word_boundary(s: &str, byte_idx: usize) -> usize {
    if byte_idx >= s.len() {
        return s.len();
    }
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let current_char_pos = chars
        .iter()
        .position(|(idx, _)| *idx >= byte_idx)
        .unwrap_or(chars.len());

    let mut i = current_char_pos;
    if i < chars.len() && (chars[i].1.is_alphanumeric() || chars[i].1 == '_') {
        while i < chars.len() && (chars[i].1.is_alphanumeric() || chars[i].1 == '_') {
            i += 1;
        }
    } else if i < chars.len() && !chars[i].1.is_whitespace() {
        while i < chars.len() && !chars[i].1.is_alphanumeric() && !chars[i].1.is_whitespace() {
            i += 1;
        }
    }
    while i < chars.len() && chars[i].1.is_whitespace() {
        i += 1;
    }
    if i < chars.len() {
        chars[i].0
    } else {
        s.len()
    }
}

pub fn render_prompt(
    mode: ModalMode,
    buffer: &str,
    cursor: usize,
    visual_anchor: Option<usize>,
) -> io::Result<()> {
    let mut stdout = io::stdout();
    let prompt_prefix = "\x1b[1;35morbit\x1b[0m"; // bold magenta
    let mode_badge = match mode {
        ModalMode::Insert => "\x1b[1;32m[INSERT]\x1b[0m", // bold green
        ModalMode::Normal => "\x1b[1;33m[NORMAL]\x1b[0m", // bold yellow
        ModalMode::Visual => "\x1b[1;35m[VISUAL]\x1b[0m", // bold magenta
    };

    // Terminal cursor shape: \x1b[6 q for bar, \x1b[2 q for block
    match mode {
        ModalMode::Insert => write!(stdout, "\x1b[6 q")?,
        ModalMode::Normal | ModalMode::Visual => write!(stdout, "\x1b[2 q")?,
    }

    // Build visual highlight if visual mode is active
    let display_buffer = if let Some(anchor) = visual_anchor {
        let (start, end) = (anchor.min(cursor), anchor.max(cursor));
        let mut s = String::new();
        let mut byte_idx = 0;
        for c in buffer.chars() {
            let next_byte = byte_idx + c.len_utf8();
            if byte_idx >= start && byte_idx <= end {
                s.push_str("\x1b[7m\x1b[35m");
                s.push(c);
                s.push_str("\x1b[0m");
            } else {
                s.push(c);
            }
            byte_idx = next_byte;
        }
        s
    } else {
        buffer.to_string()
    };

    // Prompt visible width: "orbit " (6) + "[INSERT]" (8) + " > " (3) = 17 columns
    let prompt_vis_len = 17;
    let cursor_char_offset = buffer[..cursor.min(buffer.len())].chars().count();
    let col = prompt_vis_len + cursor_char_offset;

    write!(
        stdout,
        "\r\x1b[2K{} {} > {}\r",
        prompt_prefix, mode_badge, display_buffer
    )?;
    if col > 0 {
        write!(stdout, "\x1b[{}C", col)?;
    }
    stdout.flush()?;
    Ok(())
}

pub fn load_history_file(path: &Path) -> Vec<String> {
    if let Ok(content) = std::fs::read_to_string(path) {
        content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    } else {
        Vec::new()
    }
}

pub fn append_history_file(path: &Path, line: &str) -> io::Result<()> {
    use std::fs::OpenOptions;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

pub fn read_modal_line(
    history: &mut Vec<String>,
    history_file: Option<&Path>,
) -> Result<String, ReadlineError> {
    let _guard = RawModeGuard::new().map_err(ReadlineError::Io)?;
    let mut state = ModalPromptState::new();

    let mut history_index = history.len();
    let mut draft_buffer = String::new();

    render_prompt(state.mode, &state.buffer, state.cursor, state.visual_anchor)
        .map_err(ReadlineError::Io)?;

    loop {
        let ev = event::read().map_err(ReadlineError::Io)?;
        let key = match ev {
            Event::Key(k) if k.kind == KeyEventKind::Press => k,
            _ => continue,
        };

        // Global interrupt checks
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            let mut stdout = io::stdout();
            let _ = write!(stdout, "\r\x1b[2K^C\r\n");
            let _ = stdout.flush();
            return Err(ReadlineError::Interrupted);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('d')
            && state.buffer.is_empty()
        {
            let mut stdout = io::stdout();
            let _ = write!(stdout, "\r\x1b[2K\r\n");
            let _ = stdout.flush();
            return Err(ReadlineError::Eof);
        }

        match state.mode {
            ModalMode::Insert => match key.code {
                KeyCode::Esc => {
                    state.mode = ModalMode::Normal;
                    state.visual_anchor = None;
                    state.pending_key = None;
                    if state.cursor > 0 && state.cursor == state.buffer.len() {
                        state.cursor = prev_char_boundary(&state.buffer, state.cursor);
                    }
                }
                KeyCode::Enter => {
                    let mut stdout = io::stdout();
                    let _ = write!(stdout, "\r\n");
                    let _ = stdout.flush();
                    let trimmed = state.buffer.trim();
                    if !trimmed.is_empty() && history.last().map(|s| s.as_str()) != Some(trimmed) {
                        history.push(trimmed.to_string());
                        if let Some(path) = history_file {
                            let _ = append_history_file(path, trimmed);
                        }
                    }
                    return Ok(state.buffer);
                }
                KeyCode::Backspace => {
                    if state.cursor > 0 {
                        state.save_undo();
                        let prev = prev_char_boundary(&state.buffer, state.cursor);
                        state.buffer.drain(prev..state.cursor);
                        state.cursor = prev;
                    }
                }
                KeyCode::Delete => {
                    if state.cursor < state.buffer.len() {
                        state.save_undo();
                        let next = next_char_boundary(&state.buffer, state.cursor);
                        state.buffer.drain(state.cursor..next);
                    }
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.save_undo();
                    state.buffer.drain(..state.cursor);
                    state.cursor = 0;
                }
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if state.cursor > 0 {
                        state.save_undo();
                        let prev = prev_word_boundary(&state.buffer, state.cursor);
                        state.buffer.drain(prev..state.cursor);
                        state.cursor = prev;
                    }
                }
                KeyCode::Left => {
                    state.cursor = prev_char_boundary(&state.buffer, state.cursor);
                }
                KeyCode::Right => {
                    state.cursor = next_char_boundary(&state.buffer, state.cursor);
                }
                KeyCode::Home => {
                    state.cursor = 0;
                }
                KeyCode::End => {
                    state.cursor = state.buffer.len();
                }
                KeyCode::Up => {
                    if history_index > 0 {
                        if history_index == history.len() {
                            draft_buffer = state.buffer.clone();
                        }
                        history_index -= 1;
                        state.buffer = history[history_index].clone();
                        state.cursor = state.buffer.len();
                    }
                }
                KeyCode::Down => {
                    if history_index + 1 < history.len() {
                        history_index += 1;
                        state.buffer = history[history_index].clone();
                        state.cursor = state.buffer.len();
                    } else if history_index + 1 == history.len() {
                        history_index = history.len();
                        state.buffer = draft_buffer.clone();
                        state.cursor = state.buffer.len();
                    }
                }
                KeyCode::Char(c) => {
                    state.save_undo();
                    state.buffer.insert(state.cursor, c);
                    state.cursor += c.len_utf8();
                }
                _ => {}
            },
            ModalMode::Normal => {
                let pending = state.pending_key.take();
                match (pending, key.code) {
                    // Pending 'd' actions (delete)
                    (Some('d'), KeyCode::Char('d')) => {
                        state.save_undo();
                        state.yank_buffer = state.buffer.clone();
                        state.buffer.clear();
                        state.cursor = 0;
                    }
                    (Some('d'), KeyCode::Char('w')) => {
                        state.save_undo();
                        let next = next_word_boundary(&state.buffer, state.cursor);
                        state.yank_buffer = state.buffer[state.cursor..next].to_string();
                        state.buffer.drain(state.cursor..next);
                    }
                    (Some('d'), KeyCode::Char('$')) => {
                        state.save_undo();
                        state.yank_buffer = state.buffer[state.cursor..].to_string();
                        state.buffer.truncate(state.cursor);
                    }
                    (Some('d'), KeyCode::Char('0')) => {
                        state.save_undo();
                        state.yank_buffer = state.buffer[..state.cursor].to_string();
                        state.buffer.drain(..state.cursor);
                        state.cursor = 0;
                    }

                    // Pending 'c' actions (change)
                    (Some('c'), KeyCode::Char('c')) => {
                        state.save_undo();
                        state.yank_buffer = state.buffer.clone();
                        state.buffer.clear();
                        state.cursor = 0;
                        state.mode = ModalMode::Insert;
                    }
                    (Some('c'), KeyCode::Char('w')) => {
                        state.save_undo();
                        let next = next_word_boundary(&state.buffer, state.cursor);
                        state.yank_buffer = state.buffer[state.cursor..next].to_string();
                        state.buffer.drain(state.cursor..next);
                        state.mode = ModalMode::Insert;
                    }
                    (Some('c'), KeyCode::Char('$')) => {
                        state.save_undo();
                        state.yank_buffer = state.buffer[state.cursor..].to_string();
                        state.buffer.truncate(state.cursor);
                        state.mode = ModalMode::Insert;
                    }

                    // Pending 'y' actions (yank)
                    (Some('y'), KeyCode::Char('y')) => {
                        state.yank_buffer = state.buffer.clone();
                    }

                    // Pending 'r' actions (replace single character)
                    (Some('r'), KeyCode::Char(c)) => {
                        if state.cursor < state.buffer.len() {
                            state.save_undo();
                            let next = next_char_boundary(&state.buffer, state.cursor);
                            state.buffer.drain(state.cursor..next);
                            state.buffer.insert(state.cursor, c);
                        }
                    }

                    // No pending action, handle standard Normal keys
                    (None, KeyCode::Char('d')) => {
                        state.pending_key = Some('d');
                    }
                    (None, KeyCode::Char('c')) => {
                        state.pending_key = Some('c');
                    }
                    (None, KeyCode::Char('y')) => {
                        state.pending_key = Some('y');
                    }
                    (None, KeyCode::Char('r')) => {
                        state.pending_key = Some('r');
                    }

                    (None, KeyCode::Enter) => {
                        let mut stdout = io::stdout();
                        let _ = write!(stdout, "\r\n");
                        let _ = stdout.flush();
                        let trimmed = state.buffer.trim();
                        if !trimmed.is_empty()
                            && history.last().map(|s| s.as_str()) != Some(trimmed)
                        {
                            history.push(trimmed.to_string());
                            if let Some(path) = history_file {
                                let _ = append_history_file(path, trimmed);
                            }
                        }
                        return Ok(state.buffer);
                    }
                    (None, KeyCode::Char('i')) => {
                        state.mode = ModalMode::Insert;
                    }
                    (None, KeyCode::Char('a')) => {
                        if state.cursor < state.buffer.len() {
                            state.cursor = next_char_boundary(&state.buffer, state.cursor);
                        }
                        state.mode = ModalMode::Insert;
                    }
                    (None, KeyCode::Char('I')) => {
                        state.cursor = 0;
                        state.mode = ModalMode::Insert;
                    }
                    (None, KeyCode::Char('A')) => {
                        state.cursor = state.buffer.len();
                        state.mode = ModalMode::Insert;
                    }
                    (None, KeyCode::Char('o') | KeyCode::Char('O')) => {
                        state.save_undo();
                        state.buffer.clear();
                        state.cursor = 0;
                        state.mode = ModalMode::Insert;
                    }
                    (None, KeyCode::Char('v') | KeyCode::Char('V')) => {
                        state.mode = ModalMode::Visual;
                        state.visual_anchor = Some(state.cursor);
                    }
                    (None, KeyCode::Char('x')) => {
                        if state.cursor < state.buffer.len() {
                            state.save_undo();
                            let next = next_char_boundary(&state.buffer, state.cursor);
                            state.yank_buffer = state.buffer[state.cursor..next].to_string();
                            state.buffer.drain(state.cursor..next);
                            if state.cursor >= state.buffer.len() && !state.buffer.is_empty() {
                                state.cursor = prev_char_boundary(&state.buffer, state.cursor);
                            }
                        }
                    }
                    (None, KeyCode::Char('X')) => {
                        if state.cursor > 0 {
                            state.save_undo();
                            let prev = prev_char_boundary(&state.buffer, state.cursor);
                            state.yank_buffer = state.buffer[prev..state.cursor].to_string();
                            state.buffer.drain(prev..state.cursor);
                            state.cursor = prev;
                        }
                    }
                    (None, KeyCode::Char('D')) => {
                        state.save_undo();
                        state.yank_buffer = state.buffer[state.cursor..].to_string();
                        state.buffer.truncate(state.cursor);
                        if state.cursor > 0 && state.cursor == state.buffer.len() {
                            state.cursor = prev_char_boundary(&state.buffer, state.cursor);
                        }
                    }
                    (None, KeyCode::Char('C')) => {
                        state.save_undo();
                        state.yank_buffer = state.buffer[state.cursor..].to_string();
                        state.buffer.truncate(state.cursor);
                        state.mode = ModalMode::Insert;
                    }
                    (None, KeyCode::Char('s')) => {
                        if state.cursor < state.buffer.len() {
                            state.save_undo();
                            let next = next_char_boundary(&state.buffer, state.cursor);
                            state.yank_buffer = state.buffer[state.cursor..next].to_string();
                            state.buffer.drain(state.cursor..next);
                        }
                        state.mode = ModalMode::Insert;
                    }
                    (None, KeyCode::Char('S')) => {
                        state.save_undo();
                        state.yank_buffer = state.buffer.clone();
                        state.buffer.clear();
                        state.cursor = 0;
                        state.mode = ModalMode::Insert;
                    }
                    (None, KeyCode::Char('p')) => {
                        if !state.yank_buffer.is_empty() {
                            state.save_undo();
                            let insert_pos = if state.cursor < state.buffer.len() {
                                next_char_boundary(&state.buffer, state.cursor)
                            } else {
                                state.cursor
                            };
                            state.buffer.insert_str(insert_pos, &state.yank_buffer);
                            state.cursor = insert_pos + state.yank_buffer.len();
                        }
                    }
                    (None, KeyCode::Char('P')) => {
                        if !state.yank_buffer.is_empty() {
                            state.save_undo();
                            state.buffer.insert_str(state.cursor, &state.yank_buffer);
                            state.cursor += state.yank_buffer.len();
                        }
                    }
                    (None, KeyCode::Char('u')) => {
                        state.undo();
                    }
                    (None, KeyCode::Char('h') | KeyCode::Left) => {
                        state.cursor = prev_char_boundary(&state.buffer, state.cursor);
                    }
                    (None, KeyCode::Char('l') | KeyCode::Right) => {
                        if state.cursor < state.buffer.len() {
                            state.cursor = next_char_boundary(&state.buffer, state.cursor);
                        }
                    }
                    (None, KeyCode::Char('w')) => {
                        state.cursor = next_word_boundary(&state.buffer, state.cursor);
                    }
                    (None, KeyCode::Char('b')) => {
                        state.cursor = prev_word_boundary(&state.buffer, state.cursor);
                    }
                    (None, KeyCode::Char('0') | KeyCode::Char('^') | KeyCode::Home) => {
                        state.cursor = 0;
                    }
                    (None, KeyCode::Char('$') | KeyCode::End) => {
                        if !state.buffer.is_empty() {
                            state.cursor = prev_char_boundary(&state.buffer, state.buffer.len());
                        } else {
                            state.cursor = 0;
                        }
                    }
                    (None, KeyCode::Char('k') | KeyCode::Up) => {
                        if history_index > 0 {
                            if history_index == history.len() {
                                draft_buffer = state.buffer.clone();
                            }
                            history_index -= 1;
                            state.buffer = history[history_index].clone();
                            state.cursor = state.buffer.len();
                        }
                    }
                    (None, KeyCode::Char('j') | KeyCode::Down) => {
                        if history_index + 1 < history.len() {
                            history_index += 1;
                            state.buffer = history[history_index].clone();
                            state.cursor = state.buffer.len();
                        } else if history_index + 1 == history.len() {
                            history_index = history.len();
                            state.buffer = draft_buffer.clone();
                            state.cursor = state.buffer.len();
                        }
                    }
                    (None, KeyCode::Esc) => {
                        state.pending_key = None;
                    }
                    _ => {}
                }
            }
            ModalMode::Visual => {
                let anchor = state.visual_anchor.unwrap_or(state.cursor);
                let (start, end) = (anchor.min(state.cursor), anchor.max(state.cursor));
                let end_boundary = next_char_boundary(&state.buffer, end);

                match key.code {
                    KeyCode::Esc | KeyCode::Char('v') | KeyCode::Char('V') => {
                        state.mode = ModalMode::Normal;
                        state.visual_anchor = None;
                    }
                    KeyCode::Char('h') | KeyCode::Left => {
                        state.cursor = prev_char_boundary(&state.buffer, state.cursor);
                    }
                    KeyCode::Char('l') | KeyCode::Right => {
                        if state.cursor < state.buffer.len() {
                            state.cursor = next_char_boundary(&state.buffer, state.cursor);
                        }
                    }
                    KeyCode::Char('w') => {
                        state.cursor = next_word_boundary(&state.buffer, state.cursor);
                    }
                    KeyCode::Char('b') => {
                        state.cursor = prev_word_boundary(&state.buffer, state.cursor);
                    }
                    KeyCode::Char('0') | KeyCode::Home => {
                        state.cursor = 0;
                    }
                    KeyCode::Char('$') | KeyCode::End => {
                        state.cursor = state.buffer.len().saturating_sub(1);
                    }
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        state.yank_buffer = state.buffer[start..end_boundary].to_string();
                        state.mode = ModalMode::Normal;
                        state.visual_anchor = None;
                        state.cursor = start;
                    }
                    KeyCode::Char('d') | KeyCode::Char('x') => {
                        state.save_undo();
                        state.yank_buffer = state.buffer[start..end_boundary].to_string();
                        state.buffer.drain(start..end_boundary);
                        state.mode = ModalMode::Normal;
                        state.visual_anchor = None;
                        state.cursor = start.min(state.buffer.len());
                    }
                    KeyCode::Char('c') => {
                        state.save_undo();
                        state.yank_buffer = state.buffer[start..end_boundary].to_string();
                        state.buffer.drain(start..end_boundary);
                        state.mode = ModalMode::Insert;
                        state.visual_anchor = None;
                        state.cursor = start.min(state.buffer.len());
                    }
                    _ => {}
                }
            }
        }

        render_prompt(state.mode, &state.buffer, state.cursor, state.visual_anchor)
            .map_err(ReadlineError::Io)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_char_boundaries() {
        let text = "hello 😊 world";
        assert_eq!(next_char_boundary(text, 0), 1);
        // "hello " is 6 bytes. '😊' is 4 bytes (indices 6..10)
        assert_eq!(next_char_boundary(text, 6), 10);
        assert_eq!(prev_char_boundary(text, 10), 6);
        assert_eq!(prev_char_boundary(text, 6), 5);
    }

    #[test]
    fn test_word_boundaries() {
        let text = "alpha beta_gamma delta";
        assert_eq!(next_word_boundary(text, 0), 6); // "beta_gamma"
        assert_eq!(next_word_boundary(text, 6), 17); // "delta"
        assert_eq!(prev_word_boundary(text, 17), 6);
        assert_eq!(prev_word_boundary(text, 6), 0);
    }

    #[test]
    fn test_modal_state_undo() {
        let mut state = ModalPromptState::new();
        state.buffer = "first".to_string();
        state.cursor = 5;
        state.save_undo();

        state.buffer = "second".to_string();
        state.cursor = 6;

        assert!(state.undo());
        assert_eq!(state.buffer, "first");
        assert_eq!(state.cursor, 5);
    }

    #[test]
    fn test_visual_mode_yank_and_delete() {
        let mut state = ModalPromptState::new();
        state.buffer = "hello world".to_string();
        state.cursor = 0;
        state.mode = ModalMode::Visual;
        state.visual_anchor = Some(0);
        state.cursor = 4; // 'h' to 'o'

        let end_bound = next_char_boundary(&state.buffer, 4);
        state.yank_buffer = state.buffer[0..end_bound].to_string();
        assert_eq!(state.yank_buffer, "hello");

        state.buffer.drain(0..end_bound);
        assert_eq!(state.buffer, " world");
    }
}
