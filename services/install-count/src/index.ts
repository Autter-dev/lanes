// The Cloudflare Worker: the handler wired to Postgres (directly, or through Hyperdrive) and to
// Cloudflare's rate limiter. See README.md to host it.

import { Client } from "pg";
import { handle } from "./handler";
import { forget, record, type Db } from "./store";

export interface Env {
  /** A Postgres connection string: `npx wrangler secret put DATABASE_URL`. */
  DATABASE_URL?: string;
  /** Optional: Hyperdrive in front of the same database, for faster connections. */
  HYPERDRIVE?: { connectionString: string };
  /** Optional: Cloudflare's rate limiter. Keyed by the caller's address, which is never stored. */
  LIMITER?: { limit(options: { key: string }): Promise<{ success: boolean }> };
}

async function withDb(env: Env, work: (db: Db) => Promise<void>): Promise<void> {
  const connectionString = env.HYPERDRIVE?.connectionString ?? env.DATABASE_URL;
  if (!connectionString) {
    throw new Error("no database: run `npx wrangler secret put DATABASE_URL`, or bind HYPERDRIVE");
  }
  const client = new Client({ connectionString });
  await client.connect();
  try {
    await work(client);
  } finally {
    await client.end();
  }
}

export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    return handle(request, {
      async allow(req) {
        if (!env.LIMITER) return true;
        const key = req.headers.get("cf-connecting-ip") ?? "unknown";
        return (await env.LIMITER.limit({ key })).success;
      },
      later(work) {
        // Only the error's own message: never the request or anything about the caller.
        ctx.waitUntil(
          work.catch((e: unknown) =>
            console.error(`install count: ${e instanceof Error ? e.message : String(e)}`),
          ),
        );
      },
      record: (ping) => withDb(env, (db) => record(db, ping)),
      forget: (id) => withDb(env, (db) => forget(db, id)),
    });
  },
};
