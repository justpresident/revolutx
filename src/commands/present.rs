//! The presentation seam: render a [`CommandOutput`] for a surface.

use crate::error::{Error, Result};

use super::output::CommandOutput;

/// Renders a [`CommandOutput`] to the text a surface emits.
///
/// Two implementations exist: a human/table presenter (the CLI's, reused by the
/// interactive REPL) and the [`JsonPresenter`] below (the CLI's `--json` mode and
/// the MCP tool-result text). A surface picks one and prints what it returns.
pub trait Presenter {
    /// Render `output`.
    fn present(&self, output: &CommandOutput) -> Result<String>;
}

/// Pretty-JSON presenter: serializes the output's inner value. Shared by the
/// CLI's `--json` mode and the MCP tool-result text, so both emit identical
/// bytes.
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonPresenter;

impl Presenter for JsonPresenter {
    fn present(&self, output: &CommandOutput) -> Result<String> {
        serde_json::to_string_pretty(output).map_err(Error::Serialize)
    }
}
