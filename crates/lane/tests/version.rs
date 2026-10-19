//! `lane --version`, word for word. The Installers workflow compares this line to check that each
//! channel installed the version it asked for, release candidates (0.1.0-rc.1) included.

use std::process::Command;

#[test]
fn version_names_the_version_and_nothing_else() {
    let dir = std::env::temp_dir().join(format!("lane-version-test-{}", std::process::id()));
    for flag in ["--version", "-V"] {
        let out = Command::new(env!("CARGO_BIN_EXE_lane"))
            .arg(flag)
            .env("LANE_CONFIG_DIR", &dir)
            .env("LANE_STATE_DIR", dir.join("state"))
            .env("LANE_TELEMETRY", "0")
            .env("LANE_DAEMON", "0")
            .output()
            .expect("lane runs");
        assert!(out.status.success(), "{flag}: {out:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("lane {} (by Autter)\n", env!("CARGO_PKG_VERSION")),
            "{flag}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
