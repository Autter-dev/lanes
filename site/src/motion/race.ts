// The hero race. Scroll progress through the sticky hero drives every agent forward: two reach the
// same function, Lanes flags it, and fix-auth finishes and is promoted. One variable per lane
// (--f, 0 to 1) moves both its dot and its trail; the caption narrates the state.
import { gsap } from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';
import { onPage, prefersReducedMotion } from '../engine';

gsap.registerPlugin(ScrollTrigger);

const captions = [
  'Four agents, four lanes, one repo. Each lane runs its own copy of the app.',
  'fix-auth and add-tests just reached the same function. Lanes flags it now, while both agents are still working, instead of at merge time.',
  'fix-auth is done. lane promote turns it into one clean PR, and the other lanes keep working.',
];

// [start, at the warning, after the warning, at the finish] for each lane
const path = [
  [0.16, 0.5, 0.5, 1],
  [0.1, 0.5, 0.5, 0.64],
  [0.28, 0.52, 0.6, 0.74],
  [0.06, 0.4, 0.48, 0.62],
];

export const startRace = () => onPage(() => {
  const hero = document.querySelector<HTMLElement>('[data-race]');
  if (!hero || prefersReducedMotion()) return;
  const lanes = [...hero.querySelectorAll<HTMLElement>('[data-lane]')];
  const warn = hero.querySelector('[data-race-warn]');
  const promote = hero.querySelector('[data-race-promote]');
  const caption = hero.querySelector<HTMLElement>('[data-race-caption]');
  const hint = hero.querySelector('.race__hint');

  lanes.forEach((el, i) => gsap.set(el, { '--f': path[i][0] }));
  if (caption) caption.textContent = captions[0];

  const tl = gsap.timeline({ defaults: { ease: 'none' } });
  lanes.forEach((el, i) => {
    tl.to(el, { '--f': path[i][1], duration: 0.45 }, 0)
      .to(el, { '--f': path[i][2], duration: 0.17 }, 0.45)
      .to(el, { '--f': path[i][3], duration: 0.26 }, 0.62);
  });
  tl.fromTo(warn, { autoAlpha: 0, y: 6 }, { autoAlpha: 1, y: 0, duration: 0.05, ease: 'power2.out' }, 0.42)
    .to(warn, { autoAlpha: 0, duration: 0.05 }, 0.64)
    .fromTo(promote, { autoAlpha: 0, x: -8 }, { autoAlpha: 1, x: 0, duration: 0.06, ease: 'power2.out' }, 0.86)
    .to(hint, { autoAlpha: 0, duration: 0.08 }, 0.05)
    .to({}, { duration: 0.08 }, 0.92);

  let shown = 0;
  ScrollTrigger.create({
    trigger: hero, start: 'top top', end: 'bottom bottom', scrub: 0.5, animation: tl,
    onUpdate: ({ progress: p }) => {
      const next = p < 0.4 ? 0 : p < 0.8 ? 1 : 2;
      if (next !== shown && caption) {
        shown = next;
        gsap.fromTo(caption, { autoAlpha: 0, y: 8 }, { autoAlpha: 1, y: 0, duration: 0.5, ease: 'power3.out', overwrite: true });
        caption.textContent = captions[next];
      }
    },
  });

  // Before the first scroll the agents are already working: each dot idles forward and back.
  hero.querySelectorAll<HTMLElement>('[data-dot]').forEach((dot, i) => {
    gsap.to(dot, { x: 3 + i, duration: 1.1 + i * 0.27, ease: 'sine.inOut', yoyo: true, repeat: -1 });
  });
});
