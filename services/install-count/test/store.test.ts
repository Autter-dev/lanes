// The SQL, run against a real local D1 (the same SQLite runtime Cloudflare uses) through
// wrangler's getPlatformProxy, starting empty and applying migrations/ like a deploy does.

import { readdirSync, readFileSync } from "node:fs";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getPlatformProxy } from "wrangler";
import { forget, record } from "../src/store";

const ping = { id: "0123456789abcdef0123456789abcdef", version: "0.0.7", os: "macos", arch: "aarch64", channel: "homebrew" as const };
const here = (path: string) => new URL(path, import.meta.url);

/** SQL statements in a file, comments dropped. */
function statements(file: URL): string[] {
  return readFileSync(file, "utf8")
    .replace(/--.*$/gm, "")
    .split(";")
    .map((s) => s.trim())
    .filter(Boolean);
}

describe("with D1", () => {
  let proxy: Awaited<ReturnType<typeof getPlatformProxy<{ DB: D1Database }>>>;
  let db: D1Database;

  beforeAll(async () => {
    proxy = await getPlatformProxy<{ DB: D1Database }>({
      configPath: here("../wrangler.jsonc").pathname.replace(/^\/([A-Za-z]:)/, "$1"),
      persist: false,
    });
    db = proxy.env.DB;
    for (const file of readdirSync(here("../migrations")).sort()) {
      for (const sql of statements(here(`../migrations/${file}`))) await db.prepare(sql).run();
    }
  }, 60_000);

  afterAll(async () => {
    await proxy?.dispose();
  });

  it("counts an install, counts its upgrade, and forgets it on request", async () => {
    await record(db, ping);
    await record(db, { ...ping, version: "0.0.8" });
    await record(db, { ...ping, id: "f".repeat(32), channel: "npm", os: "linux" });

    const install = await db.prepare("SELECT * FROM installs WHERE id = ?1").bind(ping.id).first();
    expect(install).toMatchObject({ first_version: "0.0.7", version: "0.0.8", pings: 2, channel: "homebrew" });
    expect(String(install?.first_seen)).toMatch(/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/);
    const pings = await db.prepare("SELECT version FROM pings WHERE install_id = ?1 ORDER BY rowid").bind(ping.id).all();
    expect(pings.results.map((r) => r.version)).toEqual(["0.0.7", "0.0.8"]);

    // The questions in queries.sql all run.
    const queries = statements(here("../queries.sql"));
    expect(queries.length).toBeGreaterThanOrEqual(5);
    for (const q of queries) await db.prepare(q).all();
    const recent = await db.prepare(queries[4]).first<{ installs: number }>();
    expect(recent?.installs).toBe(2);

    await forget(db, ping.id);
    const left = await db
      .prepare("SELECT (SELECT count(*) FROM installs WHERE id = ?1) + (SELECT count(*) FROM pings WHERE install_id = ?1) AS n")
      .bind(ping.id)
      .first<{ n: number }>();
    expect(left?.n).toBe(0);
    expect((await db.prepare("SELECT count(*) AS n FROM installs").first<{ n: number }>())?.n).toBe(1);
  });
});
