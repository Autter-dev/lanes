# Lanes install count

The backend for Lanes' anonymous install count: a Cloudflare Worker that keeps counts in Postgres.

Once Lanes is set up, and once after each upgrade, `lane` sends five things: a random install id, the Lanes version, the OS and CPU type, and how Lanes was installed. This Worker accepts exactly those five fields and refuses anything else. `lane telemetry` shows what a machine sends, and `lane telemetry off` or `DO_NOT_TRACK=1` stops it. Development builds of `lane`, CI and test runs never send it.

## What it keeps

Two tables, created on the first count:

| Table | One row per | Columns |
| --- | --- | --- |
| `installs` | install id | `first_seen`, `last_seen`, `first_version`, `version`, `os`, `arch`, `channel`, `pings` |
| `pings` | message received | `at`, `install_id`, `version`, `os`, `arch`, `channel` |

It never keeps IP addresses, user agents or request logs:

- Workers Logs are off in `wrangler.jsonc`. Don't turn them on, or add Logpush, for this Worker.
- The rate limiter (20 requests a minute per address) is Cloudflare's own. Addresses are counted there and never written here.

`queries.sql` has the usual questions: installs per day by channel, by OS and CPU, versions in use, upgrades, and installs heard from in the last 30 days.

## Endpoints

| Request | Does |
| --- | --- |
| `POST /v1/installs` | Counts an install. The body is the five fields as JSON, at most 1 KB. Answers `202` and writes after answering, so `lane` never waits. `400` says what's wrong with a body. |
| `DELETE /v1/installs/<id>` | Deletes everything kept about one install. `202`. |
| `GET /` | Says what this is. |

## Host it

You need Node 22, a Cloudflare account that has the `autter.dev` zone, and a Postgres database. Any Postgres works: Neon, Supabase, or your own. A hosted one's connection string usually ends in `?sslmode=require`.

```sh
cd services/install-count
npm ci
npx wrangler login
npx wrangler secret put DATABASE_URL
npm run deploy
```

Paste the connection string when `wrangler secret put` asks for it. Deploying creates `https://lanes-count.autter.dev`, its DNS record and its certificate. That address is where release builds of `lane` send the count (`ENDPOINT` in `crates/lane-core/src/telemetry.rs`). To use another address, change the `routes` pattern in `wrangler.jsonc` and `ENDPOINT` together; a test checks that they match.

The Worker creates its tables on the first count, so its database user needs permission to create tables. If it doesn't have that, create them yourself first:

```sh
psql "$DATABASE_URL" -f schema.sql
```

Check it's up:

```sh
curl https://lanes-count.autter.dev/
```

### Faster connections (optional)

Hyperdrive keeps connections to the database warm, so each count connects faster:

```sh
npx wrangler hyperdrive create lanes-count --connection-string="<the same connection string>"
```

Put the id it prints into the `hyperdrive` lines at the end of `wrangler.jsonc`, uncomment them, and run `npm run deploy` again. When `HYPERDRIVE` is bound, the Worker uses it instead of `DATABASE_URL`.

### Cost

One request per install per version. Cloudflare's free plan allows 100,000 Worker requests a day.

## Delete an install

Anyone with an install's id can delete what's kept about it. `lane telemetry --json` shows the id.

```sh
curl -X DELETE https://lanes-count.autter.dev/v1/installs/<id>
```

## Work on it

```sh
npm test          # handler, message checks, and the SQL when TEST_DATABASE_URL is set
npm run check     # TypeScript
npm run dev       # run it locally with wrangler
```

`TEST_DATABASE_URL` points the SQL tests at a real Postgres. They work in their own schema, `lanes_count_test`, and drop it afterwards. CI starts a Postgres for them on every push.
