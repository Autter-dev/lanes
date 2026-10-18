# Lanes install count

The backend for Lanes' anonymous install count: a Cloudflare Worker that keeps counts in D1, Cloudflare's SQLite database. It runs at `https://lanes-count.autter.dev`.

Once Lanes is set up, and once after each upgrade, `lane` sends five things: a random install id, the Lanes version, the OS and CPU type, and how Lanes was installed. This Worker accepts exactly those five fields and refuses anything else. `lane telemetry` shows what a machine sends, and `lane telemetry off` or `DO_NOT_TRACK=1` stops it. Development builds of `lane`, CI and test runs never send it.

## What it keeps

Two tables in the D1 database `lanes-install-count` (`migrations/` creates them):

| Table | One row per | Columns |
| --- | --- | --- |
| `installs` | install id | `first_seen`, `last_seen`, `first_version`, `version`, `os`, `arch`, `channel`, `pings` |
| `pings` | message received | `at`, `install_id`, `version`, `os`, `arch`, `channel` |

Times are UTC, as ISO 8601 text.

It never keeps IP addresses, user agents or request logs:

- Workers Logs are off in `wrangler.jsonc`. Don't turn them on, or add Logpush, for this Worker.
- The rate limiter (20 requests a minute per address) is Cloudflare's own. Addresses are counted there and never written here.

## Endpoints

| Request | Does |
| --- | --- |
| `POST /v1/installs` | Counts an install. The body is the five fields as JSON, at most 1 KB. Answers `202` and writes after answering, so `lane` never waits. `400` says what's wrong with a body. |
| `DELETE /v1/installs/<id>` | Deletes everything kept about one install. `202`. |
| `GET /` | Says what this is. |

## Ask it questions

`queries.sql` has the usual ones: installs per day by channel, by OS and CPU, versions in use, upgrades, and installs heard from in the last 30 days. Run one with:

```sh
npx wrangler d1 execute lanes-install-count --remote --command "SELECT channel, count(*) FROM installs GROUP BY 1"
```

The D1 page in the Cloudflare dashboard has a console for the same.

## Host it

You need Node 22 and the Cloudflare account that has the `autter.dev` zone.

```sh
cd services/install-count
npm ci
npx wrangler login
```

The first time only, make the database and put its id in `wrangler.jsonc`:

```sh
npx wrangler d1 create lanes-install-count
```

Copy the `database_id` it prints into the `d1_databases` entry in `wrangler.jsonc`, and commit it. The id isn't a secret.

Then, and after every change:

```sh
npm run deploy
```

That applies any new migrations to the database, then deploys the Worker. The first deploy creates `https://lanes-count.autter.dev`, its DNS record and its certificate. That address is where release builds of `lane` send the count (`ENDPOINT` in `crates/lane-core/src/telemetry.rs`). To use another address, change the `routes` pattern in `wrangler.jsonc` and `ENDPOINT` together; a test checks that they match.

Check it's up:

```sh
curl https://lanes-count.autter.dev/
```

### Cost

One request per install per version, two rows written for each. That fits well within Cloudflare's free plan for Workers and D1.

## Delete an install

Anyone with an install's id can delete what's kept about it. `lane telemetry --json` shows the id.

```sh
curl -X DELETE https://lanes-count.autter.dev/v1/installs/<id>
```

## Work on it

```sh
npm test          # the handler, the message checks, and the SQL against a local D1
npm run check     # TypeScript
npm run dev       # run it locally, with a local D1
```

The SQL tests run on wrangler's local D1, the same SQLite runtime Cloudflare uses, starting empty and applying `migrations/` first. To change the tables, add a new numbered file to `migrations/`; never edit one that's been deployed.
