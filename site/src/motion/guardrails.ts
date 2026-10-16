// The approval round trip in the guardrails terminal: the agent's promote, the hold with its code,
// the person's approve, then the PR. Lines appear in order, commands type out, and the session
// replays while it's on screen.
import { gsap } from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';
import { onPage, prefersReducedMotion } from '../engine';

gsap.registerPlugin(ScrollTrigger);

export const startGuardrails = () => onPage(() => {
  const term = document.querySelector<HTMLElement>('[data-guard]');
  if (!term || prefersReducedMotion()) return;
  const lines = [...term.querySelectorAll<HTMLElement>('[data-guard-line]')];

  const tl = gsap.timeline({ repeat: -1, repeatDelay: 2.4, paused: true });
  tl.set(lines, { autoAlpha: 0, y: 6 });
  let at = 0.3;
  lines.forEach((line) => {
    const code = line.querySelector('code');
    if (code) {
      // A command: show the prompt, then type it out.
      const full = code.dataset.full ?? code.textContent ?? '';
      code.dataset.full = full;
      const typed = { n: 0 };
      tl.set(line, { autoAlpha: 1, y: 0 }, at)
        .fromTo(typed, { n: 0 }, {
          n: full.length, duration: full.length * 0.035, ease: 'none',
          onUpdate: () => { code.textContent = full.slice(0, Math.round(typed.n)); },
          onComplete: () => { code.textContent = full; },
        }, at);
      at += full.length * 0.035 + 0.5;
    } else {
      tl.to(line, { autoAlpha: 1, y: 0, duration: 0.3, ease: 'power3.out' }, at);
      at += line.classList.contains('is-warn') ? 1.4 : 0.45;
    }
  });

  ScrollTrigger.create({
    trigger: term, start: 'top 80%', end: 'bottom top',
    onToggle: (self) => (self.isActive ? tl.play() : tl.pause()),
  });
  return () => lines.forEach((l) => { const c = l.querySelector('code'); if (c && c.dataset.full) c.textContent = c.dataset.full; });
});
