//! Live updates for the home screen. No daemon runs yet, so a watcher thread reads Lanes' state
//! and each lane every couple of seconds, and sends a snapshot only when something changed: the
//! screen redraws only then. Once `laned` runs, its socket will send the same snapshots, and
//! [`Source`] will say so.

use lane_core::insight::{self, Details, Savings};
use lane_core::state::{Lane, LaneStats, Store};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// Everything the home screen shows about lanes, at one moment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub rows: Vec<Row>,
    pub savings: Savings,
    /// CPU cores on this machine.
    pub cores: usize,
    pub source: Source,
}

impl Snapshot {
    /// Lanes whose app answers on its port.
    pub fn running(&self) -> usize {
        self.rows.iter().filter(|r| r.details.running).count()
    }
}

/// One lane and what's measured about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub lane: Lane,
    pub details: Details,
    pub stats: Option<LaneStats>,
    /// Its folder has gone (a tool cleaned up its own worktree).
    pub gone: bool,
}

/// Where the updates come from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Source {
    /// Read from Lanes' state file and the lanes themselves, every couple of seconds.
    #[default]
    StateFile,
    /// Sent by `laned` over its socket.
    Daemon,
}

/// Read everything once: this repo's lanes (every lane outside a repo), what each cost and
/// saved, and what each looks like now.
pub fn read(repo: Option<&Path>) -> Snapshot {
    let store = Store::open_existing().ok().flatten();
    let lanes = store
        .as_ref()
        .and_then(|s| s.lanes(repo).ok())
        .unwrap_or_default();
    let stats = store
        .as_ref()
        .and_then(|s| s.lane_stats(repo).ok())
        .unwrap_or_default();
    let log = lane_core::log::read(500, None).unwrap_or_default();
    let rows: Vec<Row> = lanes
        .into_iter()
        .map(|lane| {
            let gone = !lane.path.exists();
            let details = match gone {
                true => Details::default(),
                false => insight::details(&lane, &log),
            };
            let stats = stats.get(&lane.id).cloned();
            Row {
                lane,
                details,
                stats,
                gone,
            }
        })
        .collect();
    let savings = insight::savings(
        &rows
            .iter()
            .map(|r| (&r.lane, r.stats.as_ref(), Some(&r.details)))
            .collect::<Vec<_>>(),
    );
    Snapshot {
        rows,
        savings,
        cores: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        source: Source::StateFile,
    }
}

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

/// Watch `repo`'s lanes: a snapshot now, then one each time something changes.
pub fn watch(repo: Option<PathBuf>, every: Duration) -> Watch {
    watch_with(move || read(repo.as_deref()), every)
}

/// Call `read` every `every`, and send what it returns only when it differs from the last.
fn watch_with(read: impl Fn() -> Snapshot + Send + 'static, every: Duration) -> Watch {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::spawn(move || {
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
    });
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
