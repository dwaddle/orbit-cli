use colored::*;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct Spinner {
    stop_tx: Option<watch::Sender<bool>>,
    msg_tx: Option<watch::Sender<String>>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
    start_time: Instant,
    active: Arc<AtomicBool>,
}

impl Spinner {
    /// Starts a spinner on stdout with an animated frame and elapsed time.
    /// Operates silently if stdout is not an interactive terminal.
    pub fn start(initial_message: impl Into<String>) -> Self {
        let msg = initial_message.into();
        let start_time = Instant::now();

        if !io::stdout().is_terminal() {
            return Self {
                stop_tx: None,
                msg_tx: None,
                join_handle: None,
                start_time,
                active: Arc::new(AtomicBool::new(false)),
            };
        }

        let (stop_tx, mut stop_rx) = watch::channel(false);
        let (msg_tx, msg_rx) = watch::channel(msg);
        let active = Arc::new(AtomicBool::new(true));
        let active_clone = active.clone();

        let join_handle = tokio::spawn(async move {
            let mut frame_idx = 0;
            let mut stdout = io::stdout();

            while !*stop_rx.borrow() {
                let current_msg = msg_rx.borrow().clone();
                let frame = SPINNER_FRAMES[frame_idx % SPINNER_FRAMES.len()];
                let elapsed_secs = start_time.elapsed().as_secs_f32();

                let text = format!(
                    "\r\x1b[2K{} {} {}",
                    frame.magenta().bold(),
                    current_msg.cyan(),
                    format!("({:.1}s)", elapsed_secs).dimmed()
                );

                let _ = stdout.write_all(text.as_bytes());
                let _ = stdout.flush();

                frame_idx = (frame_idx + 1) % SPINNER_FRAMES.len();

                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(80)) => {}
                    _ = stop_rx.changed() => {
                        break;
                    }
                }
            }

            let _ = stdout.write_all(b"\r\x1b[2K");
            let _ = stdout.flush();
            active_clone.store(false, Ordering::SeqCst);
        });

        Self {
            stop_tx: Some(stop_tx),
            msg_tx: Some(msg_tx),
            join_handle: Some(join_handle),
            start_time,
            active,
        }
    }

    /// Update the message displayed by the spinner in-place.
    pub fn set_message(&self, message: impl Into<String>) {
        if let Some(ref tx) = self.msg_tx {
            let _ = tx.send(message.into());
        }
    }

    /// Stops the spinner and erases the line from the terminal.
    pub fn stop_and_clear(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(handle) = self.join_handle.take() {
            handle.abort();
        }
        if io::stdout().is_terminal() {
            let mut stdout = io::stdout();
            let _ = stdout.write_all(b"\r\x1b[2K");
            let _ = stdout.flush();
        }
        self.active.store(false, Ordering::SeqCst);
    }

    /// Stops the spinner and displays a permanent completion message.
    pub fn finish_with_message(&mut self, message: &str) {
        self.stop_and_clear();
        if io::stdout().is_terminal() {
            println!("{}", message);
        }
    }

    /// Returns elapsed time since the spinner was started.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Returns whether the spinner is currently active.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop_and_clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_spinner_lifecycle() {
        let mut spinner = Spinner::start("Initial test status");
        assert!(spinner.elapsed() <= Duration::from_secs(1));
        spinner.set_message("Updated test status");
        tokio::time::sleep(Duration::from_millis(50)).await;
        spinner.stop_and_clear();
        assert!(!spinner.is_active());
    }

    #[tokio::test]
    async fn test_spinner_drop_cleans_up() {
        {
            let _spinner = Spinner::start("Drop cleanup test");
        }
        // Drops automatically and does not panic
    }
}
