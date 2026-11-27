//! `install_fatal_panic_hook` holds stdout from the restoration to the exit: another thread
//! that is still drawing cannot write to the terminal after it is restored (it would, for
//! instance, reopen a synchronized update that the shell inherits).
//!
//! The hook ends the process: the test runs itself again as a child process, which installs
//! the hook and panics, and checks what the child wrote. In its own test binary, since the
//! panic hook is process-wide.

use std::io::Write;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

/// Set in the child process.
const CHILD: &str = "TILEFAB_FATAL_HOOK_CHILD";
/// Written by the "previous hook" (the report), while the restoring hook holds stdout.
const REPORT: &str = "panic report";

#[test]
fn no_other_thread_writes_after_the_restoration() {
    if std::env::var_os(CHILD).is_some() {
        child();
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "no_other_thread_writes_after_the_restoration",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(101), "stdout: {stdout:?}");
    let (before, after) = stdout
        .split_once(REPORT)
        .unwrap_or_else(|| panic!("no report in {stdout:?}"));
    assert!(before.contains("frame"), "the drawing thread never wrote");
    assert!(
        !after.contains("frame"),
        "a frame was written after the restoration: {after:?}"
    );
}

/// Draw in a thread, then panic in another one; the hook ends the process.
fn child() -> ! {
    // The "previous hook" (color-eyre's in the demo): slow, so that a thread free to write
    // would write during it.
    std::panic::set_hook(Box::new(|_| {
        let _ = writeln!(std::io::stdout(), "{REPORT}");
        std::thread::sleep(Duration::from_millis(300));
    }));
    tilefab_term::install_fatal_panic_hook();
    let (started, wait) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let _ = writeln!(std::io::stdout(), "frame");
            let _ = started.send(());
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    wait.recv().unwrap();
    let _ = std::thread::spawn(|| panic!("deliberate panic in a task")).join();
    unreachable!("the fatal hook did not end the process");
}
