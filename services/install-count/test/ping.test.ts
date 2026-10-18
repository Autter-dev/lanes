import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { CHANNELS, parsePing } from "../src/ping";

const good = { id: "0123456789abcdef0123456789abcdef", version: "0.0.7", os: "windows", arch: "x86_64", channel: "npm" };
const body = (v: unknown) => JSON.stringify(v);

describe("parsePing", () => {
  it("takes exactly the five fields the welcome lists", () => {
    expect(parsePing(body(good))).toEqual(good);
    expect(parsePing(body({ ...good, version: "1.2.3-beta.1" }))).toEqual({ ...good, version: "1.2.3-beta.1" });
  });

  it("refuses anything more, or less", () => {
    expect(parsePing(body({ ...good, ip: "1.2.3.4" }))).toMatch(/exactly these fields/);
    expect(parsePing(body({ ...good, repo: "secret" }))).toMatch(/exactly these fields/);
    const { channel: _, ...four } = good;
    expect(parsePing(body(four))).toMatch(/exactly these fields/);
  });

  it("refuses values that don't look right", () => {
    expect(parsePing(body({ ...good, id: "not-an-id" }))).toMatch(/^id/);
    expect(parsePing(body({ ...good, id: good.id.toUpperCase() }))).toMatch(/^id/);
    expect(parsePing(body({ ...good, version: "latest" }))).toMatch(/^version/);
    expect(parsePing(body({ ...good, os: "Windows 11 Pro" }))).toMatch(/^os/);
    expect(parsePing(body({ ...good, arch: 64 }))).toMatch(/^arch/);
    expect(parsePing(body({ ...good, channel: "apt" }))).toMatch(/^channel/);
    expect(parsePing("{")).toMatch(/JSON/);
    expect(parsePing("[]")).toMatch(/object/);
  });
});

describe("the CLI and the server agree", () => {
  // Every channel `lane` can send must be one the server accepts.
  it("on the channels", () => {
    const rust = readFileSync(new URL("../../../crates/lane-core/src/telemetry.rs", import.meta.url), "utf8");
    const fn = rust.slice(rust.indexOf("pub fn channel("), rust.indexOf("pub fn ping("));
    const sent = [...fn.matchAll(/^\s*"([a-z]+)"\s*$/gm)].map((m) => m[1]);
    expect(sent.length).toBeGreaterThan(5);
    for (const c of sent) expect(CHANNELS).toContain(c);
  });

  it("on the address", () => {
    const rust = readFileSync(new URL("../../../crates/lane-core/src/telemetry.rs", import.meta.url), "utf8");
    const wrangler = readFileSync(new URL("../wrangler.jsonc", import.meta.url), "utf8");
    const host = /"pattern":\s*"([^"]+)"/.exec(wrangler)?.[1];
    expect(rust).toContain(`Some("https://${host}/v1/installs")`);
  });
});
