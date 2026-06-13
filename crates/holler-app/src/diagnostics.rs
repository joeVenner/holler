//! Crash + diagnostic logging for the bundled tray app.
//!
//! A tray agent launched from Finder has **no console**: stdout/stderr aren't
//! connected to anything the user can see, and release builds are
//! `panic = "abort"` + `strip`, so a panic vanishes without a trace — the
//! "app just exits and I have no logs" report. This module makes the process
//! debuggable without touching any existing call site:
//!
//! 1. When stderr is **not** a terminal (i.e. the bundled `.app`, not
//!    `cargo run`), redirect fds 1 and 2 to `<data_dir>/Holler/holler.log` via
//!    `dup2`. Every existing `println!`/`eprintln!` — and the panic/abort
//!    message itself — then lands in the file unchanged.
//! 2. Install a panic hook that writes a timestamped `PANIC` banner with the
//!    location and payload (to the log file directly when fds weren't
//!    redirected), flushes, then chains the default hook.
//!
//! `cargo run` in a terminal keeps console output (the redirect is skipped),
//! preserving the dev workflow that debug builds rely on.

use std::io::{IsTerminal, Write};
use std::time::{SystemTime, UNIX_EPOCH};

/// Start fresh once the log grows past this on launch — bounds the file across
/// many runs while still surviving a crash-then-relaunch within one session.
const MAX_LOG_BYTES: u64 = 1_000_000;

/// Install diagnostics: redirect std fds to the log file (bundled app only) and
/// register the panic hook. Best-effort — any failure degrades silently to the
/// prior behaviour rather than taking down launch.
pub fn init() {
    let Ok(path) = holler_config::log_path() else {
        install_panic_hook(false);
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Truncate once it exceeds the cap; otherwise append so a crash and the
    // relaunch that follows share one readable timeline.
    let truncate = std::fs::metadata(&path)
        .map(|m| m.len() > MAX_LOG_BYTES)
        .unwrap_or(false);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(!truncate)
        .truncate(truncate)
        .open(&path);

    // Only redirect for the windowed/bundled app — never when a terminal is
    // attached (that's `cargo run`, where the developer wants console output).
    let redirected = match &file {
        Ok(f) if !std::io::stderr().is_terminal() => redirect_std_fds(f),
        _ => false,
    };

    install_panic_hook(redirected);

    // Delimit this run in the log (visible in the file or the dev terminal).
    println!(
        "\n===== Holler v{} started (unix {}) =====",
        env!("CARGO_PKG_VERSION"),
        unix_secs()
    );
    let _ = std::io::stdout().flush();
}

/// Register a panic hook that records a timestamped banner before the default
/// (abort) behaviour runs. When `redirected` is false the banner is also
/// appended straight to the log file, so panics are captured even if fd
/// redirection didn't happen (failed, or a non-unix host).
fn install_panic_hook(redirected: bool) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".into());
        let banner = format!(
            "\n===== PANIC (unix {}) at {} =====\n{}\n",
            unix_secs(),
            loc,
            payload_str(info.payload())
        );

        // stderr already points at the log file when redirected; otherwise write
        // to the file directly so the panic is never lost.
        if !redirected {
            if let Ok(p) = holler_config::log_path() {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
                    let _ = f.write_all(banner.as_bytes());
                    let _ = f.flush();
                }
            }
        }
        eprint!("{banner}");
        let _ = std::io::stderr().flush();

        default(info);
    }));
}

/// Best-effort extraction of a panic payload's message.
fn payload_str(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("Box<dyn Any>")
}

/// Seconds since the Unix epoch (0 if the clock is before it). Kept dependency-
/// free — a raw epoch stamp is enough to correlate log lines and crashes.
fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Point fds 1 and 2 at the already-open log `file`. After `dup2`, stdout/stderr
/// share the file's open description, so dropping the original handle is safe.
#[cfg(unix)]
fn redirect_std_fds(file: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    // SAFETY: `fd` is a valid, open file descriptor for the lifetime of this
    // call; dup2 onto the standard descriptors is the canonical redirect and
    // touches no Rust-side invariants.
    unsafe { libc::dup2(fd, libc::STDOUT_FILENO) >= 0 && libc::dup2(fd, libc::STDERR_FILENO) >= 0 }
}

#[cfg(not(unix))]
fn redirect_std_fds(_file: &std::fs::File) -> bool {
    false
}
