// One Autter review cycle on a lane, in order: the lane's changes travel out through the gateway,
// the cloud runs code review, security checks and tests one after another, the findings travel
// back, and they land on the lane one by one, ending with the verdict. Loops while in view.
import { gsap } from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';
import { isMobile, onPage, prefersReducedMotion } from '../engine';

gsap.registerPlugin(ScrollTrigger);

export const startReview = () => onPage(() => {
  const root = document.querySelector<HTMLElement>('[data-review]');
  if (!root || prefersReducedMotion()) return;
  const out = root.querySelector<HTMLElement>('[data-review-out]')!;
  const back = root.querySelector<HTMLElement>('[data-review-back]')!;
  const bars = [...root.querySelectorAll<HTMLElement>('[data-review-check] .review__bar span')];
  const checks = [...root.querySelectorAll<HTMLElement>('[data-review-check]')];
  const findings = [...root.querySelectorAll<HTMLElement>('[data-review-findings] li')];
  const status = root.querySelector<HTMLElement>('[data-review-status]')!;
  const prop = isMobile() ? 'y' : 'x';
  const span = (el: HTMLElement) => () => (prop === 'y' ? el.parentElement!.clientHeight : el.parentElement!.clientWidth);

  const tl = gsap.timeline({ repeat: -1, repeatDelay: 1.2, paused: true });
  tl.add(() => { status.textContent = 'Waiting for changes'; root.dataset.phase = 'idle'; })
    .set(findings, { autoAlpha: 0, y: 6 })
    .set(bars, { scaleX: 0 })
    .set(checks, { opacity: 0.45 })
    .set([out, back], { autoAlpha: 0 })
    // 1. changes leave the lane through the gateway
    .fromTo(out, { [prop]: 0, autoAlpha: 1 }, { [prop]: span(out), duration: 0.9, ease: 'power1.inOut', immediateRender: false }, 0.3)
    .set(out, { autoAlpha: 0 }, 1.2)
    .add(() => { status.textContent = 'Review running'; root.dataset.phase = 'running'; }, 1.2);
  // 2. the cloud runs each check in turn
  bars.forEach((b, i) => {
    tl.to(checks[i], { opacity: 1, duration: 0.2 }, 1.25 + i * 0.55)
      .to(b, { scaleX: 1, duration: 0.5, ease: 'power2.inOut' }, 1.25 + i * 0.55);
  });
  const done = 1.25 + bars.length * 0.55;
  tl.add(() => { status.textContent = 'Review done'; root.dataset.phase = 'done'; }, done)
    // 3. findings travel back and land on the lane
    .fromTo(back, { [prop]: span(back), autoAlpha: 1 }, { [prop]: 0, duration: 0.9, ease: 'power1.inOut', immediateRender: false }, done + 0.1)
    .set(back, { autoAlpha: 0 }, done + 1.0)
    .to(findings, { autoAlpha: 1, y: 0, duration: 0.35, stagger: 0.18, ease: 'power3.out' }, done + 1.0)
    .to({}, { duration: 1.4 });

  ScrollTrigger.create({
    trigger: root, start: 'top 75%', end: 'bottom top',
    onToggle: (self) => (self.isActive ? tl.play() : tl.pause()),
  });
});
