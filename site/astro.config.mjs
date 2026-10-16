// @ts-check
import { defineConfig } from 'astro/config';
import sitemap from '@astrojs/sitemap';

// `site` is the production URL (canonical, OG, sitemap). lanes.autter.dev is a placeholder:
// change it here, in src/data/site.ts and in public/robots.txt before deploying.
export default defineConfig({
  site: 'https://lanes.autter.dev',
  integrations: [sitemap()],
  prefetch: { prefetchAll: true, defaultStrategy: 'hover' },
  build: { inlineStylesheets: 'auto' },
});
