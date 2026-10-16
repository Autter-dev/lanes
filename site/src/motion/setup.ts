// Global motion for the Lanes site: runs once on the client, before the first page initializes.
import { gsap } from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';
import { ease, onPage, onScroll, scrollToTarget, setTransition, startScroll, stopScroll } from '../engine';
import { startRace } from './race';
import { startHow } from './how';
import { startGateway } from './gateway';
import { startLoop } from './loop';
import { startReview } from './review';
import { startGuardrails } from './guardrails';
import { SplitText } from 'gsap/SplitText';

gsap.registerPlugin(ScrollTrigger, SplitText);

export function setupMotion() {
  // Page transition: a lane change. Four ink bands glide in from the left, staggered like lanes,
  // then carry on to the right off the new page.
  let destination = '';
  document.addEventListener('click', (e) => {
    const a = (e.target as HTMLElement).closest('a');
    destination = a?.textContent?.trim().replace(/\s+/g, ' ').slice(0, 40) ?? '';
  }, true);
  setTransition({
    leave: (c) => {
      const label = c.querySelector<HTMLElement>('[data-curtain-label]');
      if (label) label.textContent = destination ? `lane change: ${destination}` : 'lane change';
      return gsap.timeline()
        .set(c, { autoAlpha: 1 })
        .fromTo(c.querySelectorAll('.curtain__band'), { xPercent: -101 }, { xPercent: 0, duration: 0.7, ease: ease.inOut, stagger: 0.05 })
        .fromTo(label, { autoAlpha: 0, x: -12 }, { autoAlpha: 1, x: 0, duration: 0.3, ease: ease.soft }, '-=0.25');
    },
    enter: (c) => gsap.timeline()
      .to(c.querySelectorAll('.curtain__band'), { xPercent: 101, duration: 0.75, ease: ease.inOut, stagger: 0.05 })
      .set(c, { autoAlpha: 0 }),
  });

  // Header: solid once the page moves, steps aside on the way down, back on the way up.
  const html = document.documentElement;
  let last = 0;
  const header = ({ y }: { y: number }) => {
    html.classList.toggle('is-scrolled', y > 24);
    if (Math.abs(y - last) < 4) return;
    html.classList.toggle('header-away', y > 200 && y > last && !html.classList.contains('menu-open'));
    last = y;
  };
  onScroll(header); // the engine feeds this from Lenis, or from native scroll under reduced motion

  // Header follows the tone of the band under it.
  onPage(() => {
    document.querySelectorAll<HTMLElement>('[data-tone="ink"]').forEach((band) => {
      ScrollTrigger.create({
        trigger: band, start: 'top 36px', end: 'bottom 36px',
        onToggle: (self) => html.classList.toggle('on-ink', self.isActive),
      });
    });
    return () => html.classList.remove('on-ink', 'header-away');
  });

  startRace();
  startHow();
  startGateway();
  startLoop();
  startReview();
  startGuardrails();
  footer();
  menu();
  copy();
  notFound();
}

// Mobile menu: a dialog with a focus trap; Escape and every link close it.
function menu() {
  onPage(() => {
    const open = document.querySelector<HTMLButtonElement>('[data-menu-open]');
    const panel = document.getElementById('menu');
    if (!open || !panel) return;
    const close = panel.querySelector<HTMLButtonElement>('[data-menu-close]')!;
    const focusables = () => [...panel.querySelectorAll<HTMLElement>('a, button')];
    const html = document.documentElement;

    const show = () => {
      panel.hidden = false;
      open.setAttribute('aria-expanded', 'true');
      html.classList.add('menu-open');
      stopScroll();
      gsap.fromTo(panel.querySelectorAll('.menu__links a'), { yPercent: 60, autoAlpha: 0 }, { yPercent: 0, autoAlpha: 1, duration: 0.7, stagger: 0.05, ease: ease.out });
      focusables()[0]?.focus();
    };
    const hide = (restore = true) => {
      if (panel.hidden) return;
      panel.hidden = true;
      open.setAttribute('aria-expanded', 'false');
      html.classList.remove('menu-open');
      startScroll();
      if (restore) open.focus();
    };
    const key = (e: KeyboardEvent) => {
      if (panel.hidden) return;
      if (e.key === 'Escape') return hide();
      if (e.key !== 'Tab') return;
      const f = focusables();
      const first = f[0], lastEl = f[f.length - 1];
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); lastEl.focus(); }
      else if (!e.shiftKey && document.activeElement === lastEl) { e.preventDefault(); first.focus(); }
    };
    const onLink = () => hide(false);
    open.addEventListener('click', show);
    close.addEventListener('click', () => hide());
    panel.querySelectorAll('[data-menu-link]').forEach((a) => a.addEventListener('click', onLink));
    document.addEventListener('keydown', key);
    return () => { document.removeEventListener('keydown', key); hide(false); };
  });
}

// Copy buttons: data-copy="#selector" copies that element's text.
function copy() {
  onPage(() => {
    const buttons = [...document.querySelectorAll<HTMLButtonElement>('[data-copy]')];
    const timers: number[] = [];
    buttons.forEach((b) => b.addEventListener('click', async () => {
      const text = document.querySelector(b.dataset.copy!)?.textContent?.trim() ?? '';
      try {
        await navigator.clipboard.writeText(text);
        b.textContent = 'Copied';
      } catch {
        b.textContent = 'Select it';
      }
      timers.push(window.setTimeout(() => { b.textContent = 'Copy'; }, 1600));
    }));
    return () => timers.forEach(clearTimeout);
  });
}

// 404: name the lane that was asked for.
function notFound() {
  onPage(() => {
    const el = document.querySelector<HTMLElement>('[data-missing-lane]');
    if (!el) return;
    const name = decodeURIComponent(location.pathname).replace(/^\/+|\/+$/g, '').split('/').pop();
    if (!name) return;
    const max = matchMedia('(min-width: 900px)').matches ? 28 : 16;
    el.textContent = name.length > max ? `${name.slice(0, max)}…` : name;
    if (name.length > max) el.nextElementSibling?.setAttribute('hidden', '');
  });
}

// Footer finale: four lanes run to the edge, the links pull in, and the giant wordmark rises
// letter by letter out of the bottom of the page as you reach it. Scrubbed, so it reverses too.
function footer() {
  onPage(() => {
    const foot = document.querySelector<HTMLElement>('[data-footer]');
    const word = foot?.querySelector<HTMLElement>('[data-footer-word]');
    if (!foot || !word || !document.documentElement.classList.contains('motion')) return;
    const split = SplitText.create(word, { type: 'chars' });
    const dot = word.querySelector('.dot');
    const lanes = foot.querySelectorAll('[data-footer-lane]');
    gsap.timeline({ scrollTrigger: { trigger: foot, start: 'top 85%', end: 'bottom bottom', scrub: 0.8 } })
      .from(lanes, { scaleX: 0, transformOrigin: '0 50%', stagger: 0.08, duration: 0.6, ease: 'none' }, 0)
      .from(foot.querySelectorAll('[data-footer-links] li'), { y: 24, autoAlpha: 0, stagger: 0.05, duration: 0.4, ease: 'power2.out' }, 0.1)
      .from(split.chars.filter((c) => !c.closest('.dot')), { yPercent: 105, stagger: 0.07, duration: 0.6, ease: 'power3.out' }, 0.3)
      .from(dot, { yPercent: -260, autoAlpha: 0, duration: 0.4, ease: 'expo.out' }, 0.8);
    const top = foot.querySelector<HTMLElement>('[data-footer-top]');
    const goTop = (e: Event) => { e.preventDefault(); scrollToTarget(0); };
    top?.addEventListener('click', goTop);
    return () => { split.revert(); top?.removeEventListener('click', goTop); };
  });
}
