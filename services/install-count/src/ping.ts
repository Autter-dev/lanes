// The one message `lane` sends, and the rules it must meet. Anything beyond the five fields the
// welcome lists is refused, so the server can't start keeping more than people were told.

/** How Lanes was installed, as `lane_core::telemetry::channel` names it. */
export const CHANNELS = ["npm", "homebrew", "scoop", "winget", "cargo", "script", "source", "other"] as const;
export type Channel = (typeof CHANNELS)[number];

export interface Ping {
  /** Random, made on the user's machine: 32 lowercase hex characters. */
  id: string;
  version: string;
  os: string;
  arch: string;
  channel: Channel;
}

const FIELDS = ["arch", "channel", "id", "os", "version"];
const VERSION = /^\d{1,4}\.\d{1,4}\.\d{1,4}(?:[-+][0-9A-Za-z.-]{1,32})?$/;
const NAME = /^[a-z0-9_]{1,16}$/;

export function isInstallId(s: string): boolean {
  return /^[0-9a-f]{32}$/.test(s);
}

/** The message, or what's wrong with it, in words for the response. */
export function parsePing(body: string): Ping | string {
  let value: unknown;
  try {
    value = JSON.parse(body);
  } catch {
    return "The body must be JSON.";
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return "The body must be a JSON object.";
  }
  const v = value as Record<string, unknown>;
  if (Object.keys(v).sort().join() !== FIELDS.join()) {
    return `The body must have exactly these fields: ${FIELDS.join(", ")}.`;
  }
  const { id, version, os, arch, channel } = v;
  if (typeof id !== "string" || !isInstallId(id)) {
    return "id must be 32 lowercase hex characters.";
  }
  if (typeof version !== "string" || !VERSION.test(version)) {
    return "version must be a version number, like 0.0.7.";
  }
  if (typeof os !== "string" || !NAME.test(os)) {
    return "os must be a short lowercase name, like linux.";
  }
  if (typeof arch !== "string" || !NAME.test(arch)) {
    return "arch must be a short lowercase name, like x86_64.";
  }
  if (typeof channel !== "string" || !(CHANNELS as readonly string[]).includes(channel)) {
    return `channel must be one of: ${CHANNELS.join(", ")}.`;
  }
  return { id, version, os, arch, channel: channel as Channel };
}
