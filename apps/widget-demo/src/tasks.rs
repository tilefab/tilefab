//! Simulated tasks: each runs in its own tokio task and communicates
//! with the UI only through channel messages.

use std::time::Duration;

use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::{Instant, interval};

use crate::app::AppEvent;

/// Unique task identifier (two tasks can have the same name).
pub type TaskId = u64;

/// Progress reporting interval.
const PROGRESS_PERIOD: Duration = Duration::from_millis(100);

/// Launch a simulated task lasting a random 3 to 10 seconds, which sends
/// `Progress` regularly, then `TaskDone`. Return the chosen duration.
///
/// The channel is bounded: a `Progress` that finds it full is skipped (the next one
/// supersedes it), while `TaskDone` waits for room, as it must never be lost.
pub fn spawn(id: TaskId, tx: Sender<AppEvent>) -> Duration {
    let duration = Duration::from_millis(fastrand::u64(3_000..=10_000));
    tokio::spawn(async move {
        let start = Instant::now();
        let mut ticker = interval(PROGRESS_PERIOD);
        loop {
            ticker.tick().await;
            let ratio = (start.elapsed().as_secs_f64() / duration.as_secs_f64()).min(1.0);
            match tx.try_send(AppEvent::Progress { id, ratio }) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                // The UI has stopped: stop the task.
                Err(TrySendError::Closed(_)) => return,
            }
            if ratio >= 1.0 {
                break;
            }
        }
        let _ = tx.send(AppEvent::TaskDone(id)).await;
    });
    duration
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn task_reports_increasing_progress_then_done() {
        let (tx, mut rx) = mpsc::channel(16);
        let duration = spawn(7, tx);
        assert!(
            (3.0..=10.0).contains(&duration.as_secs_f64()),
            "{duration:?}"
        );

        let mut last = 0.0;
        loop {
            match rx.recv().await.expect("the task ended without TaskDone") {
                AppEvent::Progress { id, ratio } => {
                    assert_eq!(id, 7);
                    assert!(ratio >= last, "progress went back: {last} then {ratio}");
                    last = ratio;
                }
                AppEvent::TaskDone(id) => {
                    assert_eq!(id, 7);
                    break;
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert!(
            (last - 1.0).abs() < f64::EPSILON,
            "last progress {last}, expected 1"
        );
        // The task has ended and dropped its sender: the channel is closed.
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn task_stops_when_the_ui_is_gone() {
        let (tx, rx) = mpsc::channel(16);
        let weak = tx.downgrade();
        spawn(1, tx);
        drop(rx);
        // Well under the shortest duration (3 s): the task must stop on its first failed send.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            weak.upgrade().is_none(),
            "the task is still running after the UI is gone"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn full_queue_skips_progress_but_never_loses_done() {
        // One place, and nobody reads during the whole task (3 to 10 s): the progress
        // updates that find it full are skipped, `TaskDone` waits for room.
        let (tx, mut rx) = mpsc::channel(1);
        spawn(3, tx);
        tokio::time::sleep(Duration::from_secs(11)).await;
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        assert!(
            matches!(events.last(), Some(AppEvent::TaskDone(3))),
            "TaskDone lost: {events:?}"
        );
        assert!(
            events.len() < 10,
            "progress should have been skipped: {events:?}"
        );
    }
}
