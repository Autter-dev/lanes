//! Clone a folder tree the way `lane new` does, and print what happened as JSON.
//!
//!   cargo run --release -p lane-core --example clone_tree -- <src> <dst> [--copy] [--skip NAME]...
//!
//! Without `--copy`, files are cloned wherever the disk allows and copied where it doesn't;
//! the report says how many of each. `.git` at the top is always skipped.

use lane_core::clone::{self, Method, Options};
use std::ffi::OsString;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(src), Some(dst)) = (args.next(), args.next()) else {
        eprintln!("usage: clone_tree <src> <dst> [--copy] [--skip NAME]...");
        std::process::exit(2);
    };
    let (src, dst) = (PathBuf::from(src), PathBuf::from(dst));
    let mut method = Method::Clone;
    let mut opts = Options::default();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--copy" => method = Method::Copy,
            "--no-metadata" => opts.keep_metadata = false,
            "--threads" => {
                opts.threads = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .expect("--threads needs a number")
            }
            "--skip" => opts
                .skip
                .push(OsString::from(args.next().expect("--skip needs a name"))),
            other => {
                eprintln!("unknown option {other}");
                std::process::exit(2);
            }
        }
    }
    let parent = dst.parent().unwrap_or(&dst).to_path_buf();
    std::fs::create_dir_all(&parent).expect("destination's folder");
    let detected = clone::detect(&parent);
    match clone::clone_tree(&src, &dst, clone::backend(method).as_ref(), &opts) {
        Ok(report) => println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "disk": detected, "report": report })
            )
            .unwrap()
        ),
        Err(e) => {
            eprintln!("clone_tree: {e}");
            std::process::exit(1);
        }
    }
}
