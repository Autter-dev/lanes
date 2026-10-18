// Postgres: one row per install, and one row per message (so upgrades show over time). No IP
// addresses, user agents or anything else about the caller is ever written.

import type { Ping } from "./ping";

/** The little of a Postgres client this needs; `pg`'s Client fits it. */
export interface Db {
  query(text: string, values?: unknown[]): Promise<unknown>;
}

/** The tables, created on first use. `schema.sql` holds the same, for running by hand. */
export const SCHEMA = `
CREATE TABLE IF NOT EXISTS installs (
  id            text PRIMARY KEY,
  first_seen    timestamptz NOT NULL DEFAULT now(),
  last_seen     timestamptz NOT NULL DEFAULT now(),
  first_version text NOT NULL,
  version       text NOT NULL,
  os            text NOT NULL,
  arch          text NOT NULL,
  channel       text NOT NULL,
  pings         integer NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS pings (
  at         timestamptz NOT NULL DEFAULT now(),
  install_id text NOT NULL,
  version    text NOT NULL,
  os         text NOT NULL,
  arch       text NOT NULL,
  channel    text NOT NULL
);
CREATE INDEX IF NOT EXISTS pings_install_id ON pings (install_id);
CREATE INDEX IF NOT EXISTS pings_at ON pings (at);
`.trim();

// A new install, or a known one reporting again (an upgrade): one statement, so the two tables
// never disagree.
const RECORD = `
WITH install AS (
  INSERT INTO installs (id, first_version, version, os, arch, channel)
  VALUES ($1, $2, $2, $3, $4, $5)
  ON CONFLICT (id) DO UPDATE
    SET version = EXCLUDED.version, os = EXCLUDED.os, arch = EXCLUDED.arch,
        channel = EXCLUDED.channel, last_seen = now(), pings = installs.pings + 1
  RETURNING id
)
INSERT INTO pings (install_id, version, os, arch, channel)
SELECT id, $2, $3, $4, $5 FROM install`.trim();

// Two Workers creating the tables at once can collide inside Postgres; the loser's error is
// harmless, since the winner made them.
const ALREADY_THERE = new Set(["42P07", "23505", "42710"]);

let schemaReady = false;

async function ensureSchema(db: Db): Promise<void> {
  if (schemaReady) return;
  try {
    await db.query(SCHEMA);
  } catch (e) {
    if (!ALREADY_THERE.has((e as { code?: string }).code ?? "")) throw e;
  }
  schemaReady = true;
}

/** For tests that start from an empty database. */
export function forgetSchemaForTests(): void {
  schemaReady = false;
}

export async function record(db: Db, ping: Ping): Promise<void> {
  await ensureSchema(db);
  await db.query(RECORD, [ping.id, ping.version, ping.os, ping.arch, ping.channel]);
}

/** Delete everything kept about one install. */
export async function forget(db: Db, id: string): Promise<void> {
  await ensureSchema(db);
  await db.query("DELETE FROM pings WHERE install_id = $1", [id]);
  await db.query("DELETE FROM installs WHERE id = $1", [id]);
}
