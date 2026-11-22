//! `install_panic_hook` restores the terminal but lets the panic go on: a caught panic does
//! not end the process, and destructors run.
//!
//! In its own test binary: the panic hook is process-wide, it must not apply to other
//! tests. The hook writes the restore sequences to stdout (show the cursor, a line feed):
//! harmless on the test runner's terminal, which is not in raw mode.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Sets its flag when dropped.
struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn a_caught_panic_does_not_end_the_process_and_runs_destructors() {
    let previous_called = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&previous_called);
    // The "previous hook" (color-eyre's in the demo): the restoring hook must chain it.
    std::panic::set_hook(Box::new(move |_| flag.store(true, Ordering::SeqCst)));
    tilefab_term::install_panic_hook();

    let dropped = Arc::new(AtomicBool::new(false));
    let guard = SetOnDrop(Arc::clone(&dropped));
    let result = std::panic::catch_unwind(move || {
        let _guard = guard;
        panic!("deliberate panic in a test");
    });

    // Reaching this point at all means the hook did not exit the process.
    assert!(result.is_err(), "the panic must propagate to catch_unwind");
    assert!(
        previous_called.load(Ordering::SeqCst),
        "previous hook not called"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "destructors must run while unwinding"
    );
    drop(std::panic::take_hook());
}
