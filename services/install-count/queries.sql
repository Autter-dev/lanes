-- Questions to ask the install count. Run one with:
--   npx wrangler d1 execute lanes-install-count --remote --command "<the query>"
-- `source` is someone's own build from a checkout. Leave it out to count people's installs.

-- New installs per day, by how Lanes was installed
SELECT substr(first_seen, 1, 10) AS day, channel, count(*) AS installs
FROM installs
GROUP BY 1, 2
ORDER BY 1 DESC, 2;

-- Installs by OS and CPU type
SELECT os, arch, count(*) AS installs
FROM installs
WHERE channel <> 'source'
GROUP BY 1, 2
ORDER BY 3 DESC;

-- The version each install last reported
SELECT version, count(*) AS installs
FROM installs
WHERE channel <> 'source'
GROUP BY 1
ORDER BY 2 DESC;

-- Upgrades per day: installs reporting a version they hadn't sent before
SELECT substr(p.at, 1, 10) AS day, p.version, count(*) AS upgrades
FROM pings p
JOIN installs i ON i.id = p.install_id
WHERE p.version <> i.first_version
GROUP BY 1, 2
ORDER BY 1 DESC, 2;

-- Installs heard from in the last 30 days (set up or upgraded then)
SELECT count(*) AS installs
FROM installs
WHERE last_seen > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-30 days') AND channel <> 'source';
