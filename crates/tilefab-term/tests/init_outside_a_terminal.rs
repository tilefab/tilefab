//! `init_inline`, `init_fullscreen` and `init_with_options` refuse to start when stdin or
//! stdout is not a terminal, and touch nothing then.
//!
//! The test runs itself again as a child process whose stdin and stdout are not terminals:
//! run directly, `cargo test` in a terminal would give it the developer's terminal, which a
//! working `init_*` would put in raw mode.

use std::io::{self, Write};
use std::process::{Command, Stdio};

/// Set in the child process.
const CHILD: &str = "TILEFAB_INIT_OUTSIDE_A_TERMINAL_CHILD";
/// Written by the child once every call has failed as expected.
const DONE: &str = "every init refused";

#[test]
fn init_refuses_a_redirected_stdin_or_stdout() {
    if std::env::var_os(CHILD).is_some() {
        child();
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "init_refuses_a_redirected_stdin_or_stdout",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "child failed: {stdout:?} {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(DONE),
        "the child did not finish: {stdout:?}"
    );
    assert!(
        !stdout.contains('\x1b'),
        "escape sequence written: {stdout:?}"
    );
}

/// Each `init_*` fails with `Unsupported`, without writing anything.
fn child() {
    let options = tilefab_term::TerminalOptions::default();
    let results = [
        tilefab_term::init_inline(1).map(drop),
        tilefab_term::init_fullscreen().map(drop),
        tilefab_term::init_with_options(options).map(drop),
    ];
    for result in results {
        let error = result.expect_err("init_* started outside a terminal");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported, "{error}");
    }
    let mut out = io::stdout();
    writeln!(out, "{DONE}").unwrap();
}
