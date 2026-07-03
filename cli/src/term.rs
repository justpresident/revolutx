//! Terminal state capture/restore around raw-mode consoles.
//!
//! The agent's operator console (rustyline) holds the terminal in **raw mode**
//! while blocked in `readline`. Exit paths that end the process from another
//! thread — the idle auto-lock, a server error, the security watchdog's hard
//! exit — would otherwise leave the user's shell in raw mode (no echo, broken
//! newlines) and render the farewell message garbled, which reads as "the agent
//! died with no logs". Capture the termios once at startup and restore it on
//! every way out.

use std::io::IsTerminal;
use std::mem::MaybeUninit;

/// The saved `termios` state of stdin (`None` when stdin is not a terminal).
#[derive(Clone, Copy)]
pub struct SavedTerminal(Option<libc::termios>);

impl SavedTerminal {
    /// Captures the current terminal state; a no-op capture when stdin is not
    /// a terminal or the state cannot be read.
    pub fn capture() -> Self {
        if !std::io::stdin().is_terminal() {
            return Self(None);
        }
        // SAFETY: `tcgetattr` fills the struct on success (return value 0),
        // which is the only case where it is read.
        unsafe {
            let mut state = MaybeUninit::<libc::termios>::uninit();
            if libc::tcgetattr(libc::STDIN_FILENO, state.as_mut_ptr()) == 0 {
                Self(Some(state.assume_init()))
            } else {
                Self(None)
            }
        }
    }

    /// Restores the captured state. Safe to call from any thread, more than
    /// once, and with nothing captured (then it does nothing).
    pub fn restore(self) {
        if let Some(state) = self.0 {
            // SAFETY: `state` is a termios previously filled by `tcgetattr`;
            // TCSADRAIN lets pending output finish before switching modes.
            unsafe {
                let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSADRAIN, &raw const state);
            }
        }
    }
}

/// Restores the terminal when dropped — covers panic unwinds through the
/// owning function, where no explicit restore call runs.
pub struct RestoreOnDrop(pub SavedTerminal);

impl Drop for RestoreOnDrop {
    fn drop(&mut self) {
        self.0.restore();
    }
}
