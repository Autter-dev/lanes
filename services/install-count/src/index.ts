// The Cloudflare Worker: the handler wired to D1 and to Cloudflare's rate limiter. See README.md
// to host it.

import { handle } from "./handler";
import { forget, record } from "./store";

export interface Env {
  /** The D1 database holding the counts (`d1_databases` in wrangler.jsonc). */
  DB: D1Database;
  /** Cloudflare's rate limiter. Keyed by the caller's address, which is never stored. */
  LIMITER?: { limit(options: { key: string }): Promise<{ success: boolean }> };
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
      record: (ping) => record(env.DB, ping),
      forget: (id) => forget(env.DB, id),
    });
  },
};
