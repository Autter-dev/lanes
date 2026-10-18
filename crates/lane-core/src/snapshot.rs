//! What the terminal app shows about lanes at one moment: every lane, what's measured about it,
//! and what lanes saved. `laned` builds these once per repo and sends them to every app that's
//! watching; without it, the app reads them itself.

use crate::insight::{self, Details, Savings};
use crate::state::{Lane, LaneStats, Store};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Everything the home screen shows about lanes, at one moment.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub lane: Lane,
    pub details: Details,
    pub stats: Option<LaneStats>,
    /// Its folder has gone (a tool cleaned up its own worktree).
    pub gone: bool,
}

/// Where a snapshot came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Read from Lanes' state and the lanes themselves, by whoever asked.
    #[default]
    StateFile,
    /// Sent by `laned`.
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
    let log = crate::log::read(500, None).unwrap_or_default();
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
