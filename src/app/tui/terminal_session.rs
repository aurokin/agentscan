use std::io::{IsTerminal, Stdout, stdout};
use std::panic::{self, PanicHookInfo};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};

use super::*;

type PanicHook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

pub(super) struct TerminalSession {
    pub(super) stdout: Stdout,
    restored: Arc<AtomicBool>,
    previous_hook: Option<Arc<PanicHook>>,
}

impl TerminalSession {
    pub(super) fn enter() -> Result<Self> {
        if !std::io::stdin().is_terminal() || !stdout().is_terminal() {
            bail!("`agentscan tui` requires an interactive tty");
        }

        terminal::enable_raw_mode().context("failed to enable terminal raw mode")?;
        let mut stdout = stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to prepare TUI terminal state");
        }

        // Restore the terminal even if a panic (including on another thread, or
        // under `panic = "abort"`) prevents `Drop` from running. The guard is
        // idempotent, so it is safe when both the hook and `Drop` fire.
        let restored = Arc::new(AtomicBool::new(false));
        let previous_hook = Arc::new(panic::take_hook());
        let hook_restored = restored.clone();
        let hook_previous = previous_hook.clone();
        panic::set_hook(Box::new(move |info| {
            restore_terminal(&hook_restored);
            hook_previous(info);
        }));

        Ok(Self {
            stdout,
            restored,
            previous_hook: Some(previous_hook),
        })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Remove our panic hook and reinstall the previous one so it does not
        // leak past the session. Dropping our hook releases its `Arc` clone,
        // leaving us as the sole owner to reclaim. Skipped while unwinding:
        // `take_hook`/`set_hook` panic on a panicking thread, which would turn
        // an ordinary panic into a double-panic abort — the hook (which has
        // already restored the terminal via `claim_restore`) stays installed
        // for the dying process instead.
        if !std::thread::panicking() {
            let _ = panic::take_hook();
            if let Some(previous_hook) = self.previous_hook.take()
                && let Ok(previous_hook) = Arc::try_unwrap(previous_hook)
            {
                panic::set_hook(previous_hook);
            }
        }
        restore_terminal(&self.restored);
    }
}

fn restore_terminal(restored: &AtomicBool) {
    if !claim_restore(restored) {
        return;
    }
    let mut stdout = stdout();
    let _ = execute!(stdout, LeaveAlternateScreen, Show);
    let _ = terminal::disable_raw_mode();
}

/// Returns `true` for the first caller only, so the terminal reset runs once
/// even if the panic hook and `Drop` both fire.
fn claim_restore(restored: &AtomicBool) -> bool {
    !restored.swap(true, Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_restore_allows_a_single_reset() {
        let restored = AtomicBool::new(false);
        assert!(claim_restore(&restored));
        assert!(!claim_restore(&restored));
        assert!(!claim_restore(&restored));
    }
}
