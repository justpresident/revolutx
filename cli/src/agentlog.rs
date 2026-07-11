//! A tiny, dependency-light event log for `revolutx agent start`.
//!
//! The signing agent can die abruptly — a security-watchdog hard exit, a panic,
//! or (before rcypher 0.4.1) an uncatchable `SIGKILL` — leaving the operator's
//! console wrecked and no clue why. This appends timestamped lifecycle events to
//! a FILE so an abnormal exit is diagnosable after the fact, independent of the
//! terminal or stderr.
//!
//! SECRETS NEVER GO HERE: the vault password, the API/private keys, and the
//! one-time auth token are never logged — only lifecycle metadata. The file is
//! opened `0600` (owner-only), consistent with the process's own trust level.
//!
//! Opt-in: nothing is written unless `REVOLUTX_AGENT_LOG` names a path, so the
//! agent never writes to a predictable location unless the operator asks for it.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::{Mutex, OnceLock};

use time::OffsetDateTime;

/// Environment variable naming the log file. Unset → logging is a no-op.
const LOG_ENV: &str = "REVOLUTX_AGENT_LOG";

/// The process-global log sink: `None` inside means logging is off (var unset or
/// the path could not be opened). A `OnceLock` so the watchdog thread, the panic
/// hook, and `main` all reach the same file without threading it through.
static LOG: OnceLock<Option<Mutex<File>>> = OnceLock::new();

/// Open the log named by `REVOLUTX_AGENT_LOG` (`0600`, append) once, and record
/// that the agent started. Best-effort: a missing var or an unopenable path
/// disables logging silently — diagnostics must never break the agent.
pub fn init() {
    LOG.get_or_init(|| std::env::var_os(LOG_ENV).and_then(|path| open_sink(&path)));
    event("agent log opened");
}

/// Append one timestamped event line (`<utc> pid=<n> <msg>`). A no-op when
/// logging is off or uninitialized; never panics and never blocks on I/O errors.
pub fn event(msg: &str) {
    let Some(Some(sink)) = LOG.get() else {
        return;
    };
    write_event(
        sink,
        &format_line(OffsetDateTime::now_utc(), std::process::id(), msg),
    );
}

/// Open the append-only, owner-only (`0600`) log file, or `None` if it cannot be
/// opened. Creating with `0600` keeps the log out of reach of other users, in
/// line with the agent's own secrets-at-rest posture.
fn open_sink(path: &OsStr) -> Option<Mutex<File>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .ok()
        .map(Mutex::new)
}

/// Write one already-formatted line and flush it, so the event is durable before
/// a subsequent crash. Best-effort — I/O errors are swallowed.
fn write_event(sink: &Mutex<File>, line: &str) {
    if let Ok(mut file) = sink.lock() {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

/// One event line: `<utc> pid=<n> <msg>\n`. Pure, so the line shape is testable
/// without the global sink.
fn format_line(now: OffsetDateTime, pid: u32, msg: &str) -> String {
    format!("{} pid={pid} {msg}\n", fmt_utc(now))
}

/// ISO-8601 UTC (`YYYY-MM-DDThh:mm:ssZ`) via field accessors, so the log stays
/// readable without pulling in the `time` crate's `formatting` feature.
fn fmt_utc(t: OffsetDateTime) -> String {
    let (year, month, day) = t.to_calendar_date();
    let (hour, min, sec) = t.to_hms();
    format!(
        "{year:04}-{:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z",
        u8::from(month)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_utc_is_iso8601_zulu() {
        assert_eq!(
            fmt_utc(OffsetDateTime::from_unix_timestamp(0).unwrap()),
            "1970-01-01T00:00:00Z"
        );
        assert_eq!(
            fmt_utc(OffsetDateTime::from_unix_timestamp(1).unwrap()),
            "1970-01-01T00:00:01Z"
        );
        assert_eq!(
            fmt_utc(OffsetDateTime::from_unix_timestamp(946_684_800).unwrap()),
            "2000-01-01T00:00:00Z",
        );
    }

    #[test]
    fn format_line_has_timestamp_pid_and_message() {
        let line = format_line(
            OffsetDateTime::from_unix_timestamp(946_684_800).unwrap(),
            42,
            "listening: x",
        );
        assert_eq!(line, "2000-01-01T00:00:00Z pid=42 listening: x\n");
    }

    #[test]
    fn open_sink_appends_and_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path =
            std::env::temp_dir().join(format!("revolutx-agentlog-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let sink = open_sink(path.as_os_str()).expect("open temp log");
        write_event(&sink, "line-one\n");
        write_event(&sink, "line-two\n");

        // Appends (does not truncate between writes), and durably persisted.
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "line-one\nline-two\n"
        );
        // Owner-only: no group/other access (the security-relevant property).
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o077,
            0,
            "log must not be group/other readable (mode {mode:o})"
        );

        let _ = std::fs::remove_file(&path);
    }
}
