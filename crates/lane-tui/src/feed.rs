//! Live updates for the home screen. When it may, the app follows `laned`, which reads each
//! repo's lanes once for every app watching and sends a snapshot only when something changed.
//! Without the daemon (not installed, `LANE_DAEMON=0`, or it went away), a thread here does the
//! same: it reads Lanes' state every couple of seconds and sends only changes. Either way the
//! screen redraws only when a snapshot arrives, and [`Source`] says where it came from.

pub use lane_core::snapshot::{Row, Snapshot, Source, read};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// A running watcher. Dropping it stops the thread.
pub struct Watch {
    pub rx: Receiver<Snapshot>,
    stop: Arc<AtomicBool>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Watch `repo`'s lanes: a snapshot now, then one each time something changes. With `daemon`,
/// from `laned` (started if need be), falling back to reading the state here every `every`.
pub fn watch(repo: Option<PathBuf>, every: Duration, daemon: bool) -> Watch {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::spawn(move || {
        if daemon && let Some(updates) = from_daemon(repo.clone()) {
            for snapshot in updates {
                if stopped.load(Ordering::Relaxed) || tx.send(snapshot).is_err() {
                    return;
                }
            }
            // The daemon went away: carry on without it.
        }
        poll(&|| read(repo.as_deref()), every, &tx, &stopped);
    });
    Watch { rx, stop }
}

/// `laned`'s snapshots of `repo`, starting it if it isn't running. None when it can't be had.
fn from_daemon(repo: Option<PathBuf>) -> Option<impl Iterator<Item = Snapshot>> {
    use lane_core::daemon;
    let at = daemon::Endpoint::current().ok()?;
    let laned = daemon::laned_path()?;
    daemon::connect_or_start(&at, &laned).ok()?.watch(repo).ok()
}

/// Call `read` every `every`, and send what it returns only when it differs from the last.
fn poll(read: &dyn Fn() -> Snapshot, every: Duration, tx: &Sender<Snapshot>, stopped: &AtomicBool) {
    let mut last: Option<Snapshot> = None;
    while !stopped.load(Ordering::Relaxed) {
        let snap = read();
        if last.as_ref() != Some(&snap) {
            if tx.send(snap.clone()).is_err() {
                return;
            }
            last = Some(snap);
        }
        // Sleep in short steps, so a stop is noticed quickly.
        let step = every.min(Duration::from_millis(100));
        let mut slept = Duration::ZERO;
        while slept < every && !stopped.load(Ordering::Relaxed) {
            std::thread::sleep(step);
            slept += step;
        }
    }
}

/// [`poll`] on a thread of its own, for tests.
#[cfg(test)]
fn watch_with(read: impl Fn() -> Snapshot + Send + 'static, every: Duration) -> Watch {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::spawn(move || poll(&read, every, &tx, &stopped));
    Watch { rx, stop }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn the_watcher_sends_only_what_changed() {
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = reads.clone();
        // The same snapshot for the first five reads, then one with a different core count.
        let watch = watch_with(
            move || Snapshot {
                cores: match counted.fetch_add(1, Ordering::SeqCst) {
                    0..5 => 4,
                    _ => 8,
                },
                ..Snapshot::default()
            },
            Duration::from_millis(5),
        );
        let first = watch.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(first.cores, 4);
        let second = watch.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(second.cores, 8);
        while reads.load(Ordering::SeqCst) < 12 {
            std::thread::sleep(Duration::from_millis(5));
        }
        // Read a dozen times, sent twice.
        assert!(watch.rx.try_recv().is_err());
    }

    #[test]
    fn dropping_the_watcher_stops_it() {
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = reads.clone();
        let watch = watch_with(
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
                Snapshot::default()
            },
            Duration::from_millis(5),
        );
        watch.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(watch);
        std::thread::sleep(Duration::from_millis(50));
        let after = reads.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(reads.load(Ordering::SeqCst), after);
    }
}
