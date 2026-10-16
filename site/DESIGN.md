# Lanes website: design plan

## Brief

- **Subject:** Lanes, an open source CLI and daemon (Rust) by Autter. It gives every coding agent
  its own instant, ready-to-run copy of a repo ("a lane") with its own ports, URL and database.
- **Audience:** developers who already run several coding agents (Claude Code, Cursor, Codex,
  Gemini CLI and others) on one repo, and the leads of their teams.
- **Job:** explain how Lanes helps in under ten seconds, then get people started (install, star,
  read the README). Second job: introduce the optional Lanes gateway on Autter, without implying
  Autter is required.
- **Source of truth:** the repo README and CHANGELOG. Every claim on the site is already made there.
- **Direction from the owner:** the "Lanes website" design canvas: grainy, minimal, warm paper
  ground, ink bands, one mint accent, uncommon typefaces. Sections: how it helps, how it works, get
  started, the Lanes gateway, how an Autter review runs on a lane, how Lanes works with Autter.
- **Autter reviews are cloud hosted:** the lane stays local; for connected repos the gateway lets
  Autter's cloud read the lane's changes, and findings come back to the lane.
- **Assumptions (placeholders, listed in `src/data/site.ts`):**
  - production URL `https://lanes.autter.dev`
  - install commands (`brew install lane`, `cargo install lane`) are not live yet; the site says so
  - the terminal app does not exist yet; the site shows no screenshot of it

## Concept

| | A. The race (name + product mechanism) | B. Commit graph (formal) | C. Terminal transcript (material) |
|---|---|---|---|
| Idea | The page is four lanes; scrolling runs the agents, they nearly collide, Lanes warns, and the winner is promoted | Every section is a commit on a branch; the page is a git graph | The site is one terminal session; every section is a command and its output |
| Anchor | Lanes' actual mechanism: parallel lanes, the collision warning, `lane promote` | Lanes sits on top of Git | Lanes is a CLI |
| Signature | Scroll-driven hero: dots advance, two meet on the same function, a warning appears, one lane merges out | Branch lines draw themselves as you scroll | Commands type themselves |
| O / F / Fe / P | 4 / 5 / 4 / 2 | 3 / 4 / 4 / 3 | 2 / 4 / 5 / 4 |

**Pick A.** It is the product's own story (parallel work, a warning before the clash, one clean PR),
so it cannot be moved to another tool. Steal from C for small details only: real commands in mono
where a command is the content.

```
Idea:        Every agent gets its own lane, and the page shows the race.
Metaphor:    a running track seen from above: parallel lanes, a finish line, one winner.
Signature:   the hero is a sticky stage of four lanes. Scrolling drives the agents forward; two
             reach the same function at the same moment and Lanes flags it; the mint lane keeps
             going and is promoted out of the pack into one clean PR. (driver: src/motion/race.ts)
Emotion:     calm control. Busy, but nothing is crashing.
Motion verb: glide in lane. Things move horizontally along lines at steady speed, never bounce.
Screenshot:  the hero mid-scroll: four hairline lanes, dots at different distances, the small
             warning tag between lanes 01 and 02.
```

## Art direction

**Palette** (from the canvas; mint is Autter's own brand dot, which is why an accent this bright is
allowed on ink: it ties Lanes to its maker, and it marks one thing only, the lane you're watching)

| Role | Hex | Use |
|---|---|---|
| ground | `#F2F1EC` | paper, page ground |
| ink | `#0E0E0D` | text, dark bands, curtain |
| muted | `#62615B` | secondary text on paper (5.6:1) |
| line | `#D9D7CF` | 1 px rules on paper |
| accent | `#0FB98F` | dots and lines on paper (graphics only, never text) |
| accent-ink | `#08765A` | accent text and focus ring on paper (5.0:1) |
| on-ink | `#EDECE6` / muted `#9A998F` / line `#2A2A27` / accent `#2CFFD0` | dark bands |

Grain: one fixed layer, a 256 px PNG tile of black and white specks at low alpha (no blend mode,
so it reads on paper and ink alike and costs nothing on scroll).

**Type** (chosen to be uncommon; the site never names them)
- Supreme (Fontshare, FFL), 400, 500, 700: display 700, tracking -0.055em at mega size to -0.02em
  at m, leading 0.9. Body 400 at 17 to 18 px, leading 1.5.
- Martian Mono (Google, OFL), 400 and 500: only for content that is code or a lane label
  (commands, lane and agent names, ports). Its advance is 0.7em; the diagrams size pills from it.

**Grid & rhythm:** 12 columns, gutter = outer margin, max 1440. Text hangs from column 1 and
column 7. Section padding moods: tight (hero meta), airy (helps, start), full (hero stage, footer).

**Shape:** pills for buttons and the install field (999 px), 20 px for the one terminal card, 0
elsewhere. No shadows, no gradients.

**Motion language:** expo.out for entrances, expo.inOut for the curtain. Dots glide at constant
speed (`ease: none`) because they are agents working. Page transition: "lane change": four
horizontal ink bands slide in from the left, staggered 50 ms, then out to the right.

**Composition plan**

```
Home
[header] Lanes.                     How it helps  How it works  Get started  For teams   [GitHub] (Get started)

[hero: poster + sticky stage, 200svh]
 Every agent gets
 its own lane.                                          install pill / note
 ─ 01 fix-auth ─────────●──────────────────────── Claude Code
 ─ 02 add-tests ───────────●───────────────────── Codex        [both editing validateSession()]
 ─ 03 new-billing ─────●───────────────────────── Cursor
 ─ 04 refactor-db ──────────●──────────────────── Gemini CLI
                                       → 01 lifts out: "lane promote fix-auth → one clean PR"

[helps: two-column ledger, airy]   "Today" vs "With Lanes", five rows, then four short notes

[works: ink band, three tall columns, big numerals 1 2 3 (a real sequence), commands in mint]
 + agent names row

[start: asymmetric pair] heading + buttons left (cols 1-5), terminal card right (cols 7-12)

[gateway: flow figure] lanes → opt-in line → gateway → Autter's four checks; draws in that order,
 then work keeps flowing the same way (src/motion/gateway.ts)

[review: two panels + two wires] your machine | changes out, findings back | Autter cloud; one
 cycle in order: changes out, checks run, findings land, verdict last (src/motion/review.ts)

[autter loop: pinned ink band] write → check → ship → fix on one lane, return line to the start
 (src/motion/loop.ts)

[footer: finale] four lanes draw across, links settle, giant "Lanes." rises letter by letter out of
 the bottom edge, mint stop last (scrubbed)

Gateway (/gateway): poster heading, how it works 1-3, what you get, where it's going (4-row
table), what stays the same. 404: one empty lane, "No lane called <path>", link home.
```

**Genericness test, and what changed**
- The canvas had a tracked mono label above every heading: removed. Mono stays for real labels
  (lane names, commands) only.
- The "How it helps" rows had decorative 01 to 05: removed; the list is not a sequence. Numbers
  stay on the three how-it-works steps and the four install steps, which are sequences.
- Fade-up on every section: none. Only headlines split on scroll; the hero is the one big motion.
- The hero dots no longer loop on a timer: they are driven by scroll, so the race tells the
  product story in order (work, warning, promote).
