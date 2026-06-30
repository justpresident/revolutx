//! A progress spinner for the slow steps of vault unlock and creation.
//!
//! Bridges rcypher's [`UnlockProgress`] hook to an [`indicatif`] spinner on
//! stderr. rcypher brackets each slow step — the Argon2 key derivation on a
//! password attempt, or the bootstrap derivation when creating a vault — with
//! `start`/`finish`, so the spinner shows for exactly that span. It is suppressed
//! when stderr is not a terminal (piped output, CI), so logs stay clean.

use std::io::IsTerminal;
use std::time::Duration;

use indicatif::ProgressBar;
use rcypher::cli::UnlockProgress;

/// How often the spinner advances a frame.
const TICK: Duration = Duration::from_millis(100);

/// An [`UnlockProgress`] that animates a spinner on stderr during each slow step,
/// unless stderr is not an interactive terminal.
pub struct SpinnerProgress {
    /// Whether to draw at all — `false` when stderr is not a terminal.
    enabled: bool,
    /// The spinner for the step currently in progress, if any.
    current: Option<ProgressBar>,
}

impl SpinnerProgress {
    /// Creates a spinner that draws only when stderr is an interactive terminal.
    #[must_use]
    pub fn new() -> Self {
        Self {
            enabled: std::io::stderr().is_terminal(),
            current: None,
        }
    }
}

impl Default for SpinnerProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl UnlockProgress for SpinnerProgress {
    fn start(&mut self, label: &str) {
        if !self.enabled {
            return;
        }
        let bar = ProgressBar::new_spinner();
        bar.set_message(label.to_owned());
        bar.enable_steady_tick(TICK);
        self.current = Some(bar);
    }

    fn finish(&mut self) {
        if let Some(bar) = self.current.take() {
            bar.finish_and_clear();
        }
    }
}
