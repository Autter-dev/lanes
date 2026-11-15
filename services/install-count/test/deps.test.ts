import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

describe("security pins", () => {
  // The install-count worker's lockfile must keep undici on the advisory-fixed
  // release (GHSA-2m5v-pch6-74hx, GHSA-qrxm-fqhp-jwvm, GHSA-xjj6-f33q-929c,
  // GHSA-4qv8-2p9j-2685, GHSA-9fm3-97px-rjf6, GHSA-4w7w-5xrc-9h26,
  // GHSA-55wc-3pg8-jfw6, GHSA-f72v-6w45-q472).
  it("locks undici to 6.29.1", () => {
    const lock = JSON.parse(readFileSync(new URL("../package-lock.json", import.meta.url), "utf8"));
    const undici = lock.packages?.["node_modules/undici"];
    expect(undici?.version).toBe("6.29.1");
  });
});
