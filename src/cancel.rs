//! Ctrl-C during a chat turn stops that turn instead of killing wut.
//!
//! The handler only raises a flag; the stream reader, tool runner, and search
//! walks check it at their next step. A second Ctrl-C before the turn notices
//! exits right away, so a connection that never sends a byte cannot trap you.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::Error;

static REQUESTED: AtomicBool = AtomicBool::new(false);

/// Installs the handler. Only interactive sessions call this; one-shot runs
/// keep the default, where Ctrl-C ends the process.
pub fn install() {
    let _ = ctrlc::set_handler(|| {
        if REQUESTED.swap(true, Ordering::SeqCst) {
            // The terminal echoed ^C; end the line before leaving.
            eprintln!();
            std::process::exit(130);
        }
    });
}

pub fn reset() {
    REQUESTED.store(false, Ordering::SeqCst);
}

pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}

/// Fails with the cancellation error once Ctrl-C has been pressed.
pub fn check() -> crate::error::Result<()> {
    if requested() {
        Err(cancelled())
    } else {
        Ok(())
    }
}

pub fn cancelled() -> Error {
    Error::new("cancelled", "the question was not sent again or saved")
}
