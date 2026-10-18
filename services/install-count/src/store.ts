// D1, Cloudflare's SQLite: one row per install, and one row per message (so upgrades show over
// time). The tables come from migrations/. No IP addresses, user agents or anything else about the
// caller is ever written.

import type { Ping } from "./ping";

const NOW = "strftime('%Y-%m-%dT%H:%M:%SZ', 'now')";

// A new install, or a known one reporting again (an upgrade).
const RECORD_INSTALL = `
INSERT INTO installs (id, first_version, version, os, arch, channel)
VALUES (?1, ?2, ?2, ?3, ?4, ?5)
ON CONFLICT (id) DO UPDATE
  SET version = excluded.version, os = excluded.os, arch = excluded.arch,
      channel = excluded.channel, last_seen = ${NOW}, pings = installs.pings + 1`;

const RECORD_PING = `
INSERT INTO pings (install_id, version, os, arch, channel)
VALUES (?1, ?2, ?3, ?4, ?5)`;

/** Count a message. Both writes go in one batch, which D1 runs as a transaction. */
export async function record(db: D1Database, ping: Ping): Promise<void> {
  const values = [ping.id, ping.version, ping.os, ping.arch, ping.channel];
  await db.batch([db.prepare(RECORD_INSTALL).bind(...values), db.prepare(RECORD_PING).bind(...values)]);
}

/** Delete everything kept about one install. */
export async function forget(db: D1Database, id: string): Promise<void> {
  await db.batch([
    db.prepare("DELETE FROM pings WHERE install_id = ?1").bind(id),
    db.prepare("DELETE FROM installs WHERE id = ?1").bind(id),
  ]);
}
