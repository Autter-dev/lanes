//! What to do after a command: the same steps for people (a "Next:" block) and for scripts and
//! agents (`next` in `--json`), so both are told the same thing.

use crate::engine::Repo;
use crate::lane_env;
use crate::state::Lane;
use serde::Serialize;
use std::path::Path;

/// One step: a command to run, as you'd type it, and why in a few words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub run: String,
    pub why: String,
}

fn step(run: impl Into<String>, why: impl Into<String>) -> Step {
    Step {
        run: run.into(),
        why: why.into(),
    }
}

fn init() -> Step {
    step(
        "lane init",
        "write lane.toml, so lanes come with dependencies and their own .env",
    )
}

/// A repo with no lanes yet: set it up if it isn't, then make the first lane.
pub fn first_lane(repo: &Repo) -> Vec<Step> {
    let mut steps = Vec::new();
    if !repo.has_lane_toml() {
        steps.push(init());
    }
    steps.push(step("lane new fix-auth", "make a lane, any name will do"));
    steps
}

/// A lane that was just made or adopted: work in it, and run the app in it.
pub fn in_lane(repo: &Repo, lane: &Lane) -> Vec<Step> {
    let vars = lane_env::vars(lane);
    let var = |k: &str| {
        vars.iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let mut steps = Vec::new();
    if !repo.has_lane_toml() {
        steps.push(init());
    }
    steps.push(step(
        format!("cd {}", quote(&lane.path)),
        "start your agent here, so it works only in this lane",
    ));
    steps.push(match &repo.project.dev.command {
        // The port written out, so no shell can swallow `$PORT` on the way.
        Some(dev) => step(
            format!(
                "lane exec {} -- {}",
                lane.name,
                lane_env::expand(dev, &vars)
            ),
            format!("run the app at {}", var("LANE_URL")),
        ),
        None => step(
            format!("lane exec {} -- <command>", lane.name),
            format!("run your dev server with PORT={}", var("PORT")),
        ),
    });
    steps
}

/// After `lane init`: make a lane (or have Claude Code make one), and check in what it wrote.
pub fn after_init(to_add: &[String], claude: bool) -> Vec<Step> {
    let mut steps = vec![step(
        "lane new fix-auth",
        "make a lane; dependencies install once, the first time",
    )];
    if claude {
        steps.push(step(
            "claude -w fix-auth",
            "or start Claude Code in a lane of its own",
        ));
    }
    if !to_add.is_empty() {
        steps.push(step(
            format!("git add {}", to_add.join(" ")),
            "check it in, so teammates get the same lanes",
        ));
    }
    steps
}

/// After `lane rm`: what's left, or a new one when nothing is.
pub fn after_rm(repo: &Repo, lanes_left: usize) -> Vec<Step> {
    if lanes_left == 0 {
        first_lane(repo)
    } else {
        vec![step("lane list", "see the lanes that are left")]
    }
}

/// The steps as a block for the terminal: commands in a column, why beside them. A command too
/// long for the column (a deep folder) gets its why on the line below instead.
pub fn render(steps: &[Step]) -> String {
    const COLUMN: usize = 48;
    if steps.is_empty() {
        return String::new();
    }
    let width = steps
        .iter()
        .map(|s| s.run.chars().count())
        .filter(|&w| w <= COLUMN)
        .max()
        .unwrap_or(0);
    let mut out = String::from("Next:\n");
    for s in steps {
        if s.run.chars().count() > COLUMN {
            out.push_str(&format!("  {}\n  {:<width$}  {}\n", s.run, "", s.why));
        } else {
            out.push_str(&format!("  {:<width$}  {}\n", s.run, s.why));
        }
    }
    out
}

/// A path as it can be pasted into sh, PowerShell or cmd: in double quotes if it has spaces.
fn quote(path: &Path) -> String {
    let s = path.display().to_string();
    if s.contains(char::is_whitespace) || s.contains(['&', '(', ')', '\'', ';']) {
        format!("\"{s}\"")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use crate::state::{PortBlock, Status};
    use std::path::PathBuf;

    fn repo(dev: Option<&str>, with_toml: bool) -> (tempfile_dir::Dir, Repo) {
        let dir = tempfile_dir::Dir::new();
        if with_toml {
            std::fs::write(dir.0.join("lane.toml"), "version = 1\n").unwrap();
        }
        let mut project = Project::default();
        project.dev.command = dev.map(str::to_string);
        let repo = Repo {
            root: dir.0.clone(),
            project,
            lanes_dir: dir.0.join("lanes"),
        };
        (dir, repo)
    }

    fn lane(path: &str) -> Lane {
        Lane {
            id: "l_1".into(),
            repo: PathBuf::from("/work/app"),
            name: "fix-auth".into(),
            path: PathBuf::from(path),
            base_commit: None,
            ports: PortBlock {
                start: 4100,
                count: 10,
            },
            agent: None,
            outcome: None,
            status: Status::Ready,
            created_at: 0,
        }
    }

    /// A folder under the system temp dir, removed when dropped.
    mod tempfile_dir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Dir {
                use std::sync::atomic::{AtomicU32, Ordering};
                static N: AtomicU32 = AtomicU32::new(0);
                let p = std::env::temp_dir().join(format!(
                    "lane-next-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn a_new_lane_says_where_to_work_and_how_to_run_the_app() {
        let (_d, r) = repo(Some("npm run dev -- --port $PORT"), true);
        let steps = in_lane(&r, &lane("/work/app.lanes/fix-auth"));
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].run, "cd /work/app.lanes/fix-auth");
        assert_eq!(
            steps[1].run,
            "lane exec fix-auth -- npm run dev -- --port 4100"
        );
        assert_eq!(
            steps[1].why,
            "run the app at http://fix-auth.localhost:4100"
        );
    }

    #[test]
    fn a_repo_without_lane_toml_is_told_to_run_init_first() {
        let (_d, r) = repo(None, false);
        let steps = in_lane(&r, &lane("/work/app.lanes/fix-auth"));
        assert_eq!(steps[0].run, "lane init");
        assert_eq!(steps[2].run, "lane exec fix-auth -- <command>");
        assert!(steps[2].why.contains("PORT=4100"));
        assert_eq!(first_lane(&r)[0].run, "lane init");
    }

    #[test]
    fn paths_with_spaces_are_quoted() {
        let (_d, r) = repo(None, true);
        let steps = in_lane(&r, &lane("/Users/sam/My Code/app.lanes/fix-auth"));
        assert_eq!(steps[0].run, "cd \"/Users/sam/My Code/app.lanes/fix-auth\"");
    }

    #[test]
    fn steps_render_as_an_aligned_block() {
        let text = render(&after_init(&["lane.toml".to_string()], false));
        assert!(text.starts_with("Next:\n"));
        assert!(text.contains("  lane new fix-auth  make a lane"));
        assert!(text.contains("  git add lane.toml  check it in"));
        let text = render(&after_init(&["lane.toml".into(), "AGENTS.md".into()], true));
        assert!(text.contains("claude -w fix-auth"));
        assert!(text.contains("git add lane.toml AGENTS.md"));
        assert_eq!(render(&[]), "");

        let long = vec![
            step(format!("cd /{}", "deep/".repeat(12)), "work here"),
            step("lane list", "see them"),
        ];
        let text = render(&long);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], format!("  cd /{}", "deep/".repeat(12)));
        assert_eq!(lines[2], "             work here", "under the why column");
        assert_eq!(lines[3], "  lane list  see them");
    }

    #[test]
    fn after_rm_points_at_what_is_left() {
        let (_d, r) = repo(None, true);
        assert_eq!(after_rm(&r, 2)[0].run, "lane list");
        assert_eq!(after_rm(&r, 0)[0].run, "lane new fix-auth");
    }
}
