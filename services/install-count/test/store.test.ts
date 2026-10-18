import { readFileSync } from "node:fs";
import pg from "pg";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { forget, forgetSchemaForTests, record, SCHEMA, type Db } from "../src/store";

const ping = { id: "0123456789abcdef0123456789abcdef", version: "0.0.7", os: "macos", arch: "aarch64", channel: "homebrew" as const };

describe("without a database", () => {
  it("schema.sql matches the schema the Worker creates", () => {
    const file = readFileSync(new URL("../schema.sql", import.meta.url), "utf8");
    expect(file.trim()).toBe(SCHEMA);
  });

  it("creates the tables once, then writes one statement per count", async () => {
    forgetSchemaForTests();
    const sent: { text: string; values?: unknown[] }[] = [];
    const db: Db = { query: async (text, values) => void sent.push({ text, values }) };
    await record(db, ping);
    await record(db, { ...ping, version: "0.0.8" });
    expect(sent.map((q) => q.text === SCHEMA)).toEqual([true, false, false]);
    expect(sent[1].values).toEqual([ping.id, "0.0.7", "macos", "aarch64", "homebrew"]);
  });

  it("shrugs off another Worker having just made the tables", async () => {
    forgetSchemaForTests();
    let first = true;
    const db: Db = {
      query: async () => {
        if (first) {
          first = false;
          throw Object.assign(new Error("duplicate"), { code: "23505" });
        }
      },
    };
    await expect(record(db, ping)).resolves.toBeUndefined();
  });
});

// Against a real Postgres when TEST_DATABASE_URL is set (CI starts one). Everything happens in
// its own schema, which is dropped at the end; nothing else in that database is touched.
const url = process.env.TEST_DATABASE_URL;

describe.skipIf(!url)("with Postgres", () => {
  const client = new pg.Client({ connectionString: url });

  beforeAll(async () => {
    await client.connect();
    await client.query("DROP SCHEMA IF EXISTS lanes_count_test CASCADE");
    await client.query("CREATE SCHEMA lanes_count_test");
    await client.query("SET search_path TO lanes_count_test");
    forgetSchemaForTests();
  });

  afterAll(async () => {
    await client.query("DROP SCHEMA IF EXISTS lanes_count_test CASCADE");
    await client.end();
  });

  it("counts an install, counts its upgrade, and forgets it on request", async () => {
    await record(client, ping);
    await record(client, { ...ping, version: "0.0.8" });
    await record(client, { ...ping, id: "f".repeat(32), channel: "npm", os: "linux" });

    const { rows } = await client.query("SELECT * FROM installs WHERE id = $1", [ping.id]);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ first_version: "0.0.7", version: "0.0.8", pings: 2, channel: "homebrew" });
    const pings = await client.query("SELECT version FROM pings WHERE install_id = $1 ORDER BY at", [ping.id]);
    expect(pings.rows.map((r) => r.version)).toEqual(["0.0.7", "0.0.8"]);

    // The questions in queries.sql all run.
    const queries = readFileSync(new URL("../queries.sql", import.meta.url), "utf8")
      .replace(/--.*$/gm, "")
      .split(";")
      .map((q) => q.trim())
      .filter(Boolean);
    expect(queries.length).toBeGreaterThanOrEqual(5);
    for (const q of queries) await client.query(q);

    await forget(client, ping.id);
    const left = await client.query("SELECT (SELECT count(*) FROM installs WHERE id = $1) + (SELECT count(*) FROM pings WHERE install_id = $1) AS n", [ping.id]);
    expect(Number(left.rows[0].n)).toBe(0);
    const other = await client.query("SELECT count(*) AS n FROM installs");
    expect(Number(other.rows[0].n)).toBe(1);
  });
});
