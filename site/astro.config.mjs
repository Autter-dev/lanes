// @ts-check
import { defineConfig } from 'astro/config';
import sitemap from '@astrojs/sitemap';

// `site` is the production URL (canonical, OG, sitemap). If it ever changes, change it here,
// in src/data/site.ts and in public/robots.txt.
export default defineConfig({
  site: 'https://lanes.autter.dev',
  integrations: [sitemap()],
  prefetch: { prefetchAll: true, defaultStrategy: 'hover' },
  build: { inlineStylesheets: 'auto' },
});
