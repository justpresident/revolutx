//! Output helpers.

use serde::Serialize;

type Res = Result<(), Box<dyn std::error::Error>>;

/// Prints a value as pretty JSON.
pub fn json<T: Serialize>(value: &T) -> Res {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
