// The status lane fills as it scrolls into view, and each stop's list settles in as the line
// reaches it. Scrubbed, so it reverses on the way back up.
import { gsap } from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';
import { onPage } from '../engine';

gsap.registerPlugin(ScrollTrigger);

export const startStatus = () => onPage(() => {
  const root = document.querySelector<HTMLElement>('[data-status]');
  if (!root || !document.documentElement.classList.contains('motion')) return;
  const fill = root.querySelector('[data-status-fill]');
  const stops = [...root.querySelectorAll<HTMLElement>('[data-status-stop]')];
  const vertical = matchMedia('(max-width: 900px)').matches;

  const tl = gsap.timeline({ scrollTrigger: { trigger: root, start: 'top 80%', end: 'bottom 60%', scrub: 0.6 } });
  tl.fromTo(fill, vertical ? { scaleY: 0 } : { scaleX: 0 }, { ...(vertical ? { scaleY: 1 } : { scaleX: 1 }), ease: 'none', duration: 1 }, 0);
  stops.forEach((s, i) => {
    tl.from(s.querySelectorAll('li'), { autoAlpha: 0, y: 10, stagger: 0.03, duration: 0.18, ease: 'power2.out' }, (i / stops.length) * 0.9);
  });
});
