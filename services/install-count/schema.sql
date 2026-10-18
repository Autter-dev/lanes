CREATE TABLE IF NOT EXISTS installs (
  id            text PRIMARY KEY,
  first_seen    timestamptz NOT NULL DEFAULT now(),
  last_seen     timestamptz NOT NULL DEFAULT now(),
  first_version text NOT NULL,
  version       text NOT NULL,
  os            text NOT NULL,
  arch          text NOT NULL,
  channel       text NOT NULL,
  pings         integer NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS pings (
  at         timestamptz NOT NULL DEFAULT now(),
  install_id text NOT NULL,
  version    text NOT NULL,
  os         text NOT NULL,
  arch       text NOT NULL,
  channel    text NOT NULL
);
CREATE INDEX IF NOT EXISTS pings_install_id ON pings (install_id);
CREATE INDEX IF NOT EXISTS pings_at ON pings (at);
