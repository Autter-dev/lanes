//! `lane savings`: what lanes saved, compared with a worktree plus its own install for each. The
//! same numbers the terminal app shows, now and in total since the first lane.

use crate::{args, here, print_json, unknown_option};
use lane_core::engine::Repo;
use lane_core::error::{Error, Result};
use lane_core::insight::{self, Savings, Totals};

pub const HELP: &str = "\
lane savings: what lanes saved, compared with a worktree plus its own install for each

Usage: lane savings [--all] [--json]

Now: the disk this repo's lanes take, against what worktrees with their own installs would take,
and the installs they skipped. In total: every lane since the first, removed ones too.

Every number is measured on this machine and kept low. A worktree holds the whole tree and waits
for its own install; a cloned lane holds only the files it changed. Time saved is each skipped
install less the time the lane took to make. A lane copied in full (a disk that can't clone)
saves no disk. Nothing here leaves your machine.

Options:
  --all       Every repo on this machine (the default outside a repo)
  --json      Print {\"repo\": ..., \"now\": {...}, \"total\": {...}}
  -h, --help  Show this help
";

pub fn savings(args: &[String]) -> Result<()> {
    let (common, rest) = args::split(args)?;
    let mut all = false;
    for arg in &rest {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "--all" => all = true,
            s if s.starts_with('-') => return Err(unknown_option("savings", s)),
            s => {
                return Err(Error::usage(format!("unexpected `{s}`"))
                    .hint("`lane savings` takes no names: run it in a repo, or add `--all`."));
            }
        }
    }
    let repo = match all {
        true => None,
        false => Repo::open(&here()).ok().map(|r| r.root),
    };
    let snapshot = lane_core::snapshot::read(repo.as_deref());
    if common.json {
        print_json(&serde_json::json!({
            "repo": repo,
            "now": snapshot.savings,
            "total": snapshot.totals,
        }));
        return Ok(());
    }
    let place = match &repo {
        Some(root) => format!(
            "in {}",
            root.file_name().map_or_else(
                || root.display().to_string(),
                |n| n.to_string_lossy().into()
            )
        ),
        None => "on this machine".into(),
    };
    print!("{}", render(&place, &snapshot.savings, &snapshot.totals));
    Ok(())
}

fn plural(n: usize, one: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        n => format!("{n} {one}s"),
    }
}

/// The installs line: how many skipped, and the waiting that saved at least.
fn installs(skipped: usize, of: Option<usize>, ms_saved: u64) -> String {
    let count = match of {
        Some(of) => format!("{skipped} of {} skipped one", plural(of, "lane")),
        None => format!("{skipped} skipped"),
    };
    match (skipped, ms_saved) {
        (0, _) => "  Installs  none skipped yet\n".into(),
        (_, 0) => format!("  Installs  {count}, but making the lanes took as long\n"),
        _ => format!(
            "  Installs  {count}: at least {} of waiting saved\n",
            insight::duration(ms_saved)
        ),
    }
}

/// What `lane savings` prints for one repo (or every repo): `place` is "in shop" or "on this
/// machine".
pub fn render(place: &str, now: &Savings, total: &Totals) -> String {
    let mut out = format!("What lanes saved {place}\n\n");
    if total.lanes == 0 && now.measured == 0 {
        out.push_str(match now.lanes {
            0 => "Nothing yet. Make a lane with `lane new <name>`, and what it saves shows here.\n",
            _ => {
                "Nothing measured yet: these lanes were made before Lanes measured them. Make a \
                 lane with `lane new <name>`, and what it saves shows here.\n"
            }
        });
        return out;
    }

    match now.lanes {
        0 => out.push_str("Now: no lanes"),
        n => out.push_str(&format!("Now: {}", plural(n, "lane"))),
    }
    if now.measured < now.lanes {
        out.push_str(&format!(
            " ({} made before Lanes measured them)",
            now.lanes - now.measured
        ));
    }
    out.push('\n');
    if now.measured > 0 {
        out.push_str(&format!(
            "  Disk      {}, where worktrees with their own installs would take {}: {} saved\n",
            insight::bytes(now.as_lanes),
            insight::bytes(now.as_worktrees),
            insight::bytes(now.saved),
        ));
        out.push_str(&installs(
            now.installs_skipped,
            Some(now.measured),
            now.ms_saved,
        ));
    }

    if total.lanes > 0 {
        let since = total
            .since
            .map(|s| lane_core::log::utc(s)[..10].to_string())
            .unwrap_or_default();
        out.push_str(&format!("\nSince {since}: {}", plural(total.lanes, "lane")));
        if total.removed > 0 {
            out.push_str(&format!(", {} of them removed", total.removed));
        }
        out.push('\n');
        if total.disk_measured > 0 {
            out.push_str(&format!(
                "  Disk      {} written, where worktrees would have written {}: {} saved\n",
                insight::bytes(total.as_lanes),
                insight::bytes(total.as_worktrees),
                insight::bytes(total.not_written),
            ));
        }
        out.push_str(&installs(total.installs_skipped, None, total.ms_saved));
        let unmeasured = total.lanes - total.disk_measured;
        if unmeasured > 0 {
            out.push_str(&format!(
                "  ({} not counted for disk: {} folder had gone before Lanes could measure it)\n",
                plural(unmeasured, "lane"),
                if unmeasured == 1 { "its" } else { "each one's" },
            ));
        }
    }

    let copied = total.copied.max(now.copies);
    if copied > 0 {
        out.push_str(&format!(
            "\n{} copied in full, because this disk can't clone files: copies save no disk. Run \
             `lane doctor` to see why.\n",
            match copied {
                1 => "1 lane was".to_string(),
                n => format!("{n} lanes were"),
            }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Savings {
        Savings {
            lanes: 3,
            measured: 3,
            as_worktrees: 2_900_000_000,
            as_lanes: 41_000_000,
            saved: 2_859_000_000,
            installs_skipped: 2,
            install_ms_skipped: 96_000,
            ms_saved: 94_000,
            copies: 0,
        }
    }

    fn total() -> Totals {
        Totals {
            since: Some(1_792_257_600),
            lanes: 27,
            removed: 24,
            cloned: 27,
            copied: 0,
            disk_measured: 27,
            as_worktrees: 63_000_000_000,
            as_lanes: 1_900_000_000,
            not_written: 61_100_000_000,
            installs_skipped: 24,
            install_ms_skipped: 1_200_000,
            ms_saved: 1_140_000,
        }
    }

    #[test]
    fn says_what_lanes_saved_now_and_in_total() {
        let text = render("in shop", &now(), &total());
        assert_eq!(
            text,
            "What lanes saved in shop\n\
             \n\
             Now: 3 lanes\n  \
             Disk      41 MB, where worktrees with their own installs would take 2.9 GB: 2.9 GB saved\n  \
             Installs  2 of 3 lanes skipped one: at least 2 min of waiting saved\n\
             \n\
             Since 2026-10-17: 27 lanes, 24 of them removed\n  \
             Disk      1.9 GB written, where worktrees would have written 63 GB: 61 GB saved\n  \
             Installs  24 skipped: at least 19 min of waiting saved\n"
        );
    }

    #[test]
    fn makes_nothing_up() {
        let empty = render("in shop", &Savings::default(), &Totals::default());
        assert!(empty.contains("Nothing yet. Make a lane"), "{empty}");
        let old = Savings {
            lanes: 2,
            ..Savings::default()
        };
        let text = render("in shop", &old, &Totals::default());
        assert!(text.contains("made before Lanes measured them"), "{text}");
        assert!(!text.contains("Disk"), "{text}");

        // Copies save no disk, and a copy slower than the install saves no time either.
        let copies = Totals {
            copied: 2,
            disk_measured: 25,
            ms_saved: 0,
            ..total()
        };
        let text = render("on this machine", &Savings::default(), &copies);
        assert!(text.contains("2 lanes were copied in full"), "{text}");
        assert!(text.contains("but making the lanes took as long"), "{text}");
        assert!(
            text.contains("(2 lanes not counted for disk: each one's folder had gone"),
            "{text}"
        );
    }
}
