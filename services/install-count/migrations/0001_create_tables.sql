-- The install count's tables. Times are UTC, as ISO 8601 text (2026-10-18T14:05:00Z).
-- One row per install, and one row per message received, so upgrades show over time.

CREATE TABLE installs (
  id            TEXT PRIMARY KEY,
  first_seen    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  last_seen     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  first_version TEXT NOT NULL,
  version       TEXT NOT NULL,
  os            TEXT NOT NULL,
  arch          TEXT NOT NULL,
  channel       TEXT NOT NULL,
  pings         INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE pings (
  at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  install_id TEXT NOT NULL,
  version    TEXT NOT NULL,
  os         TEXT NOT NULL,
  arch       TEXT NOT NULL,
  channel    TEXT NOT NULL
);

CREATE INDEX pings_install_id ON pings (install_id);
CREATE INDEX pings_at ON pings (at);
