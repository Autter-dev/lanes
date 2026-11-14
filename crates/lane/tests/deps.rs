//! Dependency pins that carry a security advisory, read from the workspace lockfile.
//!
//! `Cargo.lock` is what an installed `lane` is built from, so it is the file a fix has to change.
//! The test names the fixed versions, so a stale lockfile fails here before a release ships it.

use std::path::PathBuf;

/// The version of `name` in a `Cargo.lock`, as written between quotes.
fn version_of(lock: &str, name: &str) -> String {
    let needle = format!("[[package]]\nname = \"{name}\"\nversion = \"");
    let start = lock
        .find(&needle)
        .unwrap_or_else(|| panic!("no `{name}` package in Cargo.lock"));
    let rest = &lock[start + needle.len()..];
    rest[..rest.find('"').expect("a closed version string")].to_string()
}

/// `a` at or above `b`, comparing the numeric `major.minor.patch` parts.
fn at_least(a: &str, b: &str) -> bool {
    let parts = |v: &str| {
        let mut it = v.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
        (
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
        )
    };
    parts(a) >= parts(b)
}

#[test]
fn locked_packages_are_at_or_above_the_fixed_versions() {
    let lock =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"))
            .expect("Cargo.lock is readable");

    // RUSTSEC-2026-0461 and RUSTSEC-2026-0462 (rmcp) and RUSTSEC-2026-0459 (libsqlite3-sys's
    // bundled SQLite). Keep these floors in step with the advisories.
    for (name, floor) in [("rmcp", "3.6.0"), ("libsqlite3-sys", "0.39.0")] {
        let version = version_of(&lock, name);
        assert!(
            at_least(&version, floor),
            "{name} {version} is below the fixed {floor}; bump it in Cargo.lock"
        );
    }
}
