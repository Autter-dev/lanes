// The HTTP side, apart from Cloudflare and Postgres, so every route is tested without either.
//
//   POST   /v1/installs        count an install (the body is the five fields; 202 Accepted)
//   DELETE /v1/installs/<id>   delete everything kept about one install (202 Accepted)
//   GET    /                   what this is

import { isInstallId, parsePing, type Ping } from "./ping";

export interface Deps {
  /** False when this caller has sent too much lately. */
  allow(request: Request): Promise<boolean>;
  /** Finish work after the response has gone (Cloudflare's waitUntil): `lane` never waits. */
  later(work: Promise<unknown>): void;
  record(ping: Ping): Promise<void>;
  forget(id: string): Promise<void>;
}

export const PATH = "/v1/installs";
export const MAX_BODY = 1024;

const ABOUT = `Lanes install count.

lane sends this, once it's set up and once after each upgrade: a random install id, the Lanes
version, the OS and CPU type, and how Lanes was installed. Nothing else is accepted, and no IP
address is kept. Turn it off with \`lane telemetry off\` or DO_NOT_TRACK=1.

Source: https://github.com/Autter-dev/lanes/tree/main/services/install-count
`;

export async function handle(request: Request, deps: Deps): Promise<Response> {
  const { pathname } = new URL(request.url);

  if (pathname === "/") {
    if (request.method !== "GET" && request.method !== "HEAD") return notAllowed("GET");
    return text(200, ABOUT);
  }

  if (pathname === PATH) {
    if (request.method !== "POST") return notAllowed("POST");
    if (!(await deps.allow(request))) return tooMany();
    const body = await readBody(request);
    if (body === null) return text(413, `The body must be at most ${MAX_BODY} bytes.`);
    const ping = parsePing(body);
    if (typeof ping === "string") return text(400, ping);
    deps.later(deps.record(ping));
    return new Response(null, { status: 202 });
  }

  if (pathname.startsWith(PATH + "/")) {
    if (request.method !== "DELETE") return notAllowed("DELETE");
    const id = pathname.slice(PATH.length + 1);
    if (!isInstallId(id)) return text(400, "An install id is 32 lowercase hex characters.");
    if (!(await deps.allow(request))) return tooMany();
    deps.later(deps.forget(id));
    return new Response(null, { status: 202 });
  }

  return text(404, `Nothing here. Counts go to POST ${PATH}.`);
}

/** The body as text, or null when it's bigger than MAX_BODY (stops reading there). */
async function readBody(request: Request): Promise<string | null> {
  if (Number(request.headers.get("content-length") ?? 0) > MAX_BODY) return null;
  if (!request.body) return "";
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > MAX_BODY) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const all = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) {
    all.set(c, at);
    at += c.byteLength;
  }
  return new TextDecoder().decode(all);
}

function text(status: number, body: string, headers: Record<string, string> = {}): Response {
  return new Response(body.endsWith("\n") ? body : body + "\n", {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", ...headers },
  });
}

function notAllowed(method: string): Response {
  return text(405, `Use ${method} here.`, { allow: method });
}

function tooMany(): Response {
  return text(429, "Too many requests from here. Try again in a minute.", { "retry-after": "60" });
}
