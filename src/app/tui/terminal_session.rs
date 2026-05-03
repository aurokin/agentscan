use std::io::{IsTerminal, Stdout, stdout};

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal;

use super::*;

pub(super) struct TerminalSession {
    pub(super) stdout: Stdout,
}

impl TerminalSession {
    pub(super) fn enter() -> Result<Self> {
        if !std::io::stdin().is_terminal() || !stdout().is_terminal() {
            bail!("`agentscan tui` requires an interactive tty");
        }

        terminal::enable_raw_mode().context("failed to enable terminal raw mode")?;
        let mut stdout = stdout();
        execute!(stdout, Hide).context("failed to hide cursor for TUI session")?;
        Ok(Self { stdout })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = execute!(self.stdout, Show);
        let _ = terminal::disable_raw_mode();
    }
}
