import { describe, expect, it } from "vitest";
import { handle, MAX_BODY, PATH, type Deps } from "../src/handler";
import type { Ping } from "../src/ping";

const good = { id: "0123456789abcdef0123456789abcdef", version: "0.0.7", os: "linux", arch: "aarch64", channel: "homebrew" };

/** A stand-in for Cloudflare and Postgres that keeps what it was asked to do. */
function fake(allowed = true) {
  const recorded: Ping[] = [];
  const forgotten: string[] = [];
  const pending: Promise<unknown>[] = [];
  const deps: Deps = {
    allow: async () => allowed,
    later: (work) => void pending.push(work),
    record: async (p) => void recorded.push(p),
    forget: async (id) => void forgotten.push(id),
  };
  return { deps, recorded, forgotten, settle: () => Promise.all(pending) };
}

const at = (path: string, init?: RequestInit) => new Request(`https://lanes-count.autter.dev${path}`, init);
const post = (body: string, headers: Record<string, string> = {}) =>
  at(PATH, { method: "POST", body, headers: { "content-type": "application/json", ...headers } });

describe("POST /v1/installs", () => {
  it("accepts a count and records it after answering", async () => {
    const f = fake();
    const res = await handle(post(JSON.stringify(good)), f.deps);
    expect(res.status).toBe(202);
    await f.settle();
    expect(f.recorded).toEqual([good]);
  });

  it("turns down anything else, and records nothing", async () => {
    const f = fake();
    expect((await handle(post(JSON.stringify({ ...good, ip: "1.2.3.4" })), f.deps)).status).toBe(400);
    expect((await handle(post("x".repeat(MAX_BODY + 1)), f.deps)).status).toBe(413);
    expect((await handle(at(PATH), f.deps)).status).toBe(405);
    await f.settle();
    expect(f.recorded).toEqual([]);
  });

  it("says what's wrong in words", async () => {
    const res = await handle(post(JSON.stringify({ ...good, channel: "apt" })), fake().deps);
    expect(await res.text()).toMatch(/channel must be one of/);
  });

  it("slows down a caller sending too much", async () => {
    const f = fake(false);
    const res = await handle(post(JSON.stringify(good)), f.deps);
    expect(res.status).toBe(429);
    expect(res.headers.get("retry-after")).toBe("60");
    await f.settle();
    expect(f.recorded).toEqual([]);
  });
});

describe("DELETE /v1/installs/<id>", () => {
  it("forgets an install", async () => {
    const f = fake();
    expect((await handle(at(`${PATH}/${good.id}`, { method: "DELETE" }), f.deps)).status).toBe(202);
    await f.settle();
    expect(f.forgotten).toEqual([good.id]);
  });

  it("only for a real id, and only with DELETE", async () => {
    const f = fake();
    expect((await handle(at(`${PATH}/nope`, { method: "DELETE" }), f.deps)).status).toBe(400);
    expect((await handle(at(`${PATH}/${good.id}`), f.deps)).status).toBe(405);
    await f.settle();
    expect(f.forgotten).toEqual([]);
  });
});

describe("other paths", () => {
  it("says what this is at /, and nothing elsewhere", async () => {
    const home = await handle(at("/"), fake().deps);
    expect(home.status).toBe(200);
    expect(await home.text()).toMatch(/no IP\s+address is kept/);
    expect((await handle(at("/admin"), fake().deps)).status).toBe(404);
  });
});
