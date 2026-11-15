//! Dependency pins that carry a security advisory, read from the workspace lockfiles.
//!
//! `Cargo.lock` is what an installed `lane` is built from, and `site/package-lock.json` is what
//! the website is built from, so those are the files a fix has to change. The test names the
//! fixed versions, so a stale lockfile fails here before a release ships it.

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

/// The version of the `node_modules/<name>` entry in an npm `package-lock.json`.
fn npm_version_of(lock: &str, name: &str) -> String {
    let needle = format!("\"node_modules/{name}\": {{");
    let start = lock
        .find(&needle)
        .unwrap_or_else(|| panic!("no `{name}` package in package-lock.json"));
    let rest = &lock[start + needle.len()..];
    let version = rest
        .find("\"version\": \"")
        .map(|i| &rest[i + "\"version\": \"".len()..])
        .expect("a version in the package entry");
    version[..version.find('"').expect("a closed version string")].to_string()
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

#[test]
fn site_astro_is_at_or_above_the_fixed_version() {
    // GHSA-r3h5-4585-7x3w is fixed in astro 7.4.7. Keep this floor in step with the advisory.
    let version = npm_version_of(include_str!("../../../site/package-lock.json"), "astro");
    assert!(
        at_least(&version, "7.4.7"),
        "astro {version} is below the fixed 7.4.7; bump it in site/package-lock.json"
    );
}
