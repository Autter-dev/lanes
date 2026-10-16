# Lanes website

The marketing site for Lanes: a static [Astro](https://astro.build) site with GSAP and Lenis for
motion. Design decisions (concept, palette, type, motion) live in [DESIGN.md](DESIGN.md).

## Run it

```sh
npm install
npm run dev        # local dev server
npm run build      # static site in dist/
npm run preview    # serve dist/ locally
```

Node 22 or later.

## Where things are

| Path | What |
| --- | --- |
| `src/data/site.ts` | Every visible string, the hero race and the diagram data. Edit copy here |
| `src/pages/` | `index.astro` (home), `gateway.astro`, `404.astro` |
| `src/components/HowItWorks.astro` | The scroll-driven "How it works" diagram (wide and phone versions) |
| `src/motion/race.ts` | The hero race: scroll moves the agents, flags the clash, promotes the winner |
| `src/motion/how.ts` | The diagram's scrubbed timeline, in four chapters |
| `src/motion/gateway.ts` | Gateway flow: lanes into the gateway, then out to Autter's checks |
| `src/motion/review.ts` | One cloud review cycle: changes out, checks run, findings back |
| `src/motion/loop.ts` | How it works with Autter: write, check, ship, fix |
| `src/motion/setup.ts` | Page transition, header, mobile menu, copy button, footer finale |
| `src/engine/` | Shared motion engine: one GSAP clock, Lenis, reveals, page lifecycle |
| `src/styles/` | `tokens.css` (palette, type scale), `engine.css`, `site.css` |
| `public/grain.png` | The film grain tile laid over every page |
| `public/photos/` | Two grayscale Unsplash photos; credits in `credits.json` |

## Before deploying

- **Production URL.** `https://lanes.autter.dev` is a placeholder. Change it in
  `astro.config.mjs`, `src/data/site.ts` and `public/robots.txt`.
- **Install commands.** The site says `brew install lane` and `cargo install lane` go live with
  the first release. Update `site.install` in `src/data/site.ts` when they do.
- **OG image.** `public/og.png` is a screenshot of the hero. Regenerate it after big hero changes.

Reduced motion is respected everywhere: no smooth scroll, no scrubbing, and the hero and diagram
show a designed still frame.

## Deploy

`dist/` is plain static files, so any static host works, for example `npx vercel --prod`,
`npx netlify deploy --prod --dir dist` or `npx wrangler pages deploy dist`.

Photos: H&CO and Filip Filkovic Philatz on Unsplash (Unsplash License).
