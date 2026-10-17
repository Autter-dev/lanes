//! What a repo is made of, for `lane init`: how to install its dependencies, how to start its
//! dev server and on which port, its `.env` template, Compose file and migration tool. Read-only:
//! it looks at files at the repo's top level and never runs anything.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Detected {
    /// What was recognised, in the order people would name it ("pnpm", "Next.js", ...).
    pub stack: Vec<String>,
    /// Commands for `[setup] install`.
    pub install: Vec<String>,
    pub dev: Option<DevServer>,
    /// A `.env` template, relative to the repo.
    pub env_template: Option<PathBuf>,
    pub compose: Option<PathBuf>,
    pub migrations: Vec<String>,
    /// Things worth saying that Lanes can't set up by itself yet.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DevServer {
    pub command: String,
    /// The port it uses when nothing tells it otherwise, if known.
    pub default_port: Option<u16>,
    /// Whether it reads `PORT` (or `VITE_PORT`) from the environment on its own.
    pub reads_port: bool,
}

fn has(root: &Path, name: &str) -> bool {
    root.join(name).exists()
}

/// Detect what the repo at `root` needs.
pub fn detect(root: &Path) -> Detected {
    let mut d = Detected::default();
    let package: Option<serde_json::Value> = fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());

    // JavaScript: the lockfile names the package manager.
    let pm = if has(root, "pnpm-lock.yaml") {
        Some(("pnpm", "pnpm install --frozen-lockfile", "pnpm"))
    } else if has(root, "bun.lock") || has(root, "bun.lockb") {
        Some(("Bun", "bun install --frozen-lockfile", "bun run"))
    } else if has(root, "yarn.lock") {
        if has(root, ".yarnrc.yml") {
            Some(("Yarn", "yarn install --immutable", "yarn"))
        } else {
            Some(("Yarn", "yarn install --frozen-lockfile", "yarn"))
        }
    } else if has(root, "package-lock.json") {
        Some(("npm", "npm ci", "npm run"))
    } else if package.is_some() {
        Some(("npm", "npm install", "npm run"))
    } else {
        None
    };
    if let Some((name, install, run)) = pm {
        d.stack.push(name.to_string());
        d.install.push(install.to_string());
        if let Some(pkg) = &package {
            let found = framework(pkg);
            if let Some((framework, ..)) = found {
                d.stack.push(framework.to_string());
            }
            if let Some(script) = dev_script(pkg) {
                let fixed = script_port(pkg, script);
                let (default_port, reads_port) = match found {
                    Some((_, port, reads)) => (fixed.or(port), reads && fixed.is_none()),
                    None => (fixed, false),
                };
                let mut command = format!("{run} {script}");
                // A server that won't read PORT (or whose script pins a port) is told the lane's
                // port on its command line; `lane exec` fills in `$PORT`.
                if !reads_port && (found.is_some() || fixed.is_some()) {
                    command.push_str(if name == "npm" {
                        " -- --port $PORT"
                    } else {
                        " --port $PORT"
                    });
                }
                if let Some(p) = fixed {
                    d.notes.push(format!(
                        "The `{script}` script sets port {p} itself, so Lanes passes `--port $PORT` to give each lane its own."
                    ));
                }
                d.dev = Some(DevServer {
                    command,
                    default_port,
                    reads_port,
                });
            }
        }
    }
    if has(root, "turbo.json") {
        d.stack.push("Turborepo".into());
    }
    if has(root, "nx.json") {
        d.stack.push("Nx".into());
    }

    // Python
    if has(root, "uv.lock") {
        d.stack.push("uv".into());
        d.install.push("uv sync --frozen".into());
    } else if has(root, "poetry.lock") {
        d.stack.push("Poetry".into());
        d.install.push("poetry install".into());
    } else if has(root, "requirements.txt") || has(root, "pyproject.toml") {
        d.stack.push("Python".into());
        d.notes.push("Python without uv or Poetry: add your install command to `[setup] install` in lane.toml.".into());
    }
    if has(root, "manage.py") {
        d.stack.push("Django".into());
        d.dev.get_or_insert(DevServer {
            command: "python manage.py runserver $PORT".into(),
            default_port: Some(8000),
            reads_port: false,
        });
        d.migrations.push("Django".into());
    }

    // Others
    if has(root, "Gemfile.lock") {
        d.stack.push("Bundler".into());
        d.install.push("bundle install".into());
        if has(root, "bin/rails") {
            d.stack.push("Rails".into());
            d.dev.get_or_insert(DevServer {
                command: "bin/rails server".into(),
                default_port: Some(3000),
                reads_port: true,
            });
        }
    }
    if has(root, "go.mod") {
        d.stack.push("Go".into());
        d.install.push("go mod download".into());
    }
    if has(root, "Cargo.toml") {
        let workspace =
            fs::read_to_string(root.join("Cargo.toml")).is_ok_and(|t| t.contains("[workspace]"));
        d.stack.push(
            if workspace {
                "Cargo workspace"
            } else {
                "Cargo"
            }
            .into(),
        );
        d.install.push("cargo fetch".into());
    }

    for f in [
        ".env.example",
        ".env.sample",
        ".env.template",
        "example.env",
    ] {
        if has(root, f) {
            d.env_template = Some(PathBuf::from(f));
            break;
        }
    }
    for f in [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yml",
        "docker-compose.yaml",
    ] {
        if has(root, f) {
            d.compose = Some(PathBuf::from(f));
            d.stack.push("Docker Compose".into());
            d.notes.push(format!(
                "{f}: a Compose project per lane is coming; for now lanes share it."
            ));
            break;
        }
    }
    if has(root, "prisma/schema.prisma") {
        d.migrations.push("Prisma".into());
    }
    if [
        "drizzle.config.ts",
        "drizzle.config.js",
        "drizzle.config.mjs",
    ]
    .iter()
    .any(|f| has(root, f))
    {
        d.migrations.push("Drizzle".into());
    }
    if has(root, "db/migrate") {
        d.migrations.push("Rails".into());
    }
    if has(root, "alembic.ini") {
        d.migrations.push("Alembic".into());
    }
    if !d.migrations.is_empty() {
        d.notes.push(format!(
            "{} migrations: a database per lane comes with `lane setup data`; until then lanes share your database.",
            d.migrations.join(", ")
        ));
    }
    d
}

/// The first of `dev`, `start` and `serve` that the package has as a script.
fn dev_script(pkg: &serde_json::Value) -> Option<&'static str> {
    let scripts = pkg.get("scripts")?;
    ["dev", "start", "serve"]
        .into_iter()
        .find(|s| scripts.get(*s).is_some())
}

/// A port set on the script's own command line (`--port 3001`, `-p 3001`).
fn script_port(pkg: &serde_json::Value, script: &str) -> Option<u16> {
    let line = pkg.get("scripts")?.get(script)?.as_str()?;
    let words: Vec<&str> = line.split_whitespace().collect();
    words
        .windows(2)
        .find_map(|w| match w[0] {
            "--port" | "-p" => w[1].parse().ok(),
            _ => None,
        })
        .or_else(|| {
            words
                .iter()
                .find_map(|w| w.strip_prefix("--port=").and_then(|p| p.parse().ok()))
        })
}

/// The web framework from the package's dependencies: (name, default port, reads PORT itself).
fn framework(pkg: &serde_json::Value) -> Option<(&'static str, Option<u16>, bool)> {
    let dep = |name: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|section| pkg.get(section).and_then(|s| s.get(name)).is_some())
    };
    // Most specific first: frameworks built on Vite come before Vite itself.
    let known: [(&str, &str, Option<u16>, bool); 9] = [
        ("next", "Next.js", Some(3000), true),
        ("nuxt", "Nuxt", Some(3000), true),
        ("@remix-run/dev", "Remix", Some(5173), false),
        ("@sveltejs/kit", "SvelteKit", Some(5173), false),
        ("astro", "Astro", Some(4321), false),
        ("@angular/core", "Angular", Some(4200), false),
        ("react-scripts", "Create React App", Some(3000), true),
        ("vite", "Vite", Some(5173), false),
        ("express", "Express", None, false),
    ];
    known
        .into_iter()
        .find(|(dep_name, ..)| dep(dep_name))
        .map(|(_, name, port, reads)| (name, port, reads))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lane-detect-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        for (f, text) in files {
            let p = d.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_pnpm_next_app_with_a_template_and_prisma() {
        let d = temp(
            "next",
            &[
                ("pnpm-lock.yaml", ""),
                (
                    "package.json",
                    r#"{"scripts": {"dev": "next dev --port 3001"}, "dependencies": {"next": "15"}}"#,
                ),
                (".env.example", "DATABASE_URL=\n"),
                ("prisma/schema.prisma", ""),
                ("compose.yaml", ""),
            ],
        );
        let got = detect(&d);
        assert_eq!(got.stack, ["pnpm", "Next.js", "Docker Compose"]);
        assert_eq!(got.install, ["pnpm install --frozen-lockfile"]);
        let dev = got.dev.unwrap();
        assert_eq!(
            dev.command, "pnpm dev --port $PORT",
            "the script pins 3001, so the lane's port is passed"
        );
        assert_eq!(dev.default_port, Some(3001));
        assert!(!dev.reads_port);
        assert_eq!(got.env_template.as_deref(), Some(Path::new(".env.example")));
        assert_eq!(got.migrations, ["Prisma"]);
        assert_eq!(got.notes.len(), 3, "{:?}", got.notes);
    }

    #[test]
    fn a_vite_app_on_npm_and_a_svelte_kit_app_on_yarn_berry() {
        let d = temp(
            "vite",
            &[
                ("package-lock.json", ""),
                (
                    "package.json",
                    r#"{"scripts": {"start": "vite"}, "devDependencies": {"vite": "6"}}"#,
                ),
            ],
        );
        let got = detect(&d);
        assert_eq!(got.install, ["npm ci"]);
        assert_eq!(
            got.dev.unwrap(),
            DevServer {
                command: "npm run start -- --port $PORT".into(),
                default_port: Some(5173),
                reads_port: false
            }
        );

        let d = temp(
            "kit",
            &[
                ("yarn.lock", ""),
                (".yarnrc.yml", ""),
                (
                    "package.json",
                    r#"{"scripts": {"dev": "vite dev"}, "devDependencies": {"vite": "6", "@sveltejs/kit": "2"}}"#,
                ),
            ],
        );
        let got = detect(&d);
        assert_eq!(got.stack, ["Yarn", "SvelteKit"]);
        assert_eq!(got.install, ["yarn install --immutable"]);
        assert_eq!(got.dev.unwrap().command, "yarn dev --port $PORT");

        let d = temp(
            "next-plain",
            &[
                ("package-lock.json", ""),
                (
                    "package.json",
                    r#"{"scripts": {"dev": "next dev"}, "dependencies": {"next": "15"}}"#,
                ),
            ],
        );
        let dev = detect(&d).dev.unwrap();
        assert_eq!(dev.command, "npm run dev", "Next reads PORT itself");
        assert!(dev.reads_port);
    }

    #[test]
    fn python_rust_and_go() {
        let d = temp(
            "multi",
            &[
                ("uv.lock", ""),
                ("Cargo.toml", "[workspace]\n"),
                ("go.mod", "module x\n"),
            ],
        );
        let got = detect(&d);
        assert_eq!(got.stack, ["uv", "Go", "Cargo workspace"]);
        assert_eq!(
            got.install,
            ["uv sync --frozen", "go mod download", "cargo fetch"]
        );
        assert!(got.dev.is_none());

        let d = temp("pip", &[("requirements.txt", "flask\n")]);
        let got = detect(&d);
        assert!(got.install.is_empty());
        assert!(got.notes[0].contains("[setup] install"), "{:?}", got.notes);
    }

    #[test]
    fn an_empty_repo_finds_nothing() {
        let got = detect(&temp("empty", &[("README.md", "hi\n")]));
        assert_eq!(got, Detected::default());
    }
}
