// How it works: one scrubbed timeline draws the product in four chapters while the stage is
// pinned (CSS sticky). Scroll progress through the section is the single driver: it moves the
// diagram, lights the active chapter, swaps the command and fills the progress line.
import { gsap } from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';
import { DrawSVGPlugin } from 'gsap/DrawSVGPlugin';
import { onPage } from '../engine';
import { steps } from '../data/site';

gsap.registerPlugin(ScrollTrigger, DrawSVGPlugin);

type Layout = { axis: 'cx' | 'cy'; warn: number[]; done: number[] };
const layouts: Record<'wide' | 'tall', Layout> = {
  // positions of the three agents at the warning (chapter 3) and at the end (chapter 4)
  wide: { axis: 'cx', warn: [470, 470, 420], done: [640, 520, 500] },
  tall: { axis: 'cy', warn: [340, 340, 300], done: [470, 380, 360] },
};

function build(svg: SVGSVGElement, layout: Layout) {
  const q = <T extends Element>(s: string) => [...svg.querySelectorAll<T>(s)];
  const g = (name: string) => svg.querySelector(`[data-g="${name}"]`);
  const draw = (name: string) => svg.querySelector(`[data-draw="${name}"]`);
  const dots = [0, 1, 2].map((i) => svg.querySelector(`[data-dot="${i}"]`));
  const lanes = [0, 1, 2].map((i) => g(`lane${i}`));
  const labels = [0, 1, 2].map((i) => g(`label${i}`));
  const tags = [0, 1, 2].flatMap((i) => [...(g(`tags${i}`)?.children ?? [])]);

  const tl = gsap.timeline({ defaults: { ease: 'none' } });
  // First frame: the repo and its trunk are already there; everything else waits for its chapter.
  gsap.set([...labels, ...tags, g('warn'), g('pr'), ...dots], { autoAlpha: 0 });
  gsap.set(q('[data-draw]:not([data-draw="trunk"])'), { drawSVG: '0%' });

  // 1. your repo
  tl.fromTo(svg.querySelectorAll('[data-g="repo"] text'), { autoAlpha: 0.4 }, { autoAlpha: 1, duration: 0.5, stagger: 0.08 }, 0.1)
  // 2. a lane per agent, each with its own URL, port and database
    .to([0, 1, 2].map((i) => draw(`lane${i}`)), { drawSVG: '100%', duration: 0.55, stagger: 0.1 }, 1.0)
    .to(labels, { autoAlpha: 1, duration: 0.25, stagger: 0.08 }, 1.3)
    .fromTo(tags, { autoAlpha: 0, y: 6 }, { autoAlpha: 1, y: 0, duration: 0.2, stagger: 0.035, ease: 'power2.out' }, 1.45)
    .to(dots, { autoAlpha: 1, duration: 0.15, stagger: 0.05 }, 1.55)
  // 3. two agents open the same function: Lanes says so on both lanes
    .to(dots, { attr: { [layout.axis]: (i: number) => layout.warn[i] }, duration: 0.55 }, 2.0)
    .to(draw('warnline'), { drawSVG: '100%', duration: 0.25 }, 2.35)
    .to(g('warn'), { autoAlpha: 1, duration: 0.15 }, 2.35)
    .to(g('warn'), { autoAlpha: 0, duration: 0.15 }, 3.0)
  // 4. the best lane becomes one clean PR; the others stay out of your branch
    .to(dots, { attr: { [layout.axis]: (i: number) => layout.done[i] }, duration: 0.4 }, 3.0)
    .to([lanes[1], lanes[2]], { opacity: 0.3, duration: 0.3 }, 3.25)
    .to(draw('merge'), { drawSVG: '100%', duration: 0.3 }, 3.4)
    .to(g('pr'), { autoAlpha: 1, duration: 0.2 }, 3.62)
    .fromTo(draw('check'), { drawSVG: '0%' }, { drawSVG: '100%', duration: 0.2 }, 3.7)
    .to({}, { duration: 0.1 }, 3.9);
  return tl;
}

export const startHow = () => onPage(() => {
  const section = document.querySelector<HTMLElement>('[data-how]');
  if (!section) return;
  const stage = section.querySelector<HTMLElement>('.how__stage')!;
  const cmd = section.querySelector<HTMLElement>('[data-how-cmd]')!;
  const bar = section.querySelector<HTMLElement>('[data-how-progress]');
  const chapters = [...section.querySelectorAll<HTMLElement>('[data-chapter]')];

  let state = -1;
  const setState = (i: number) => {
    if (i === state) return;
    state = i;
    stage.dataset.state = String(i);
    chapters.forEach((c, k) => c.toggleAttribute('data-done', k < i));
    cmd.textContent = steps[i].cmd;
  };

  const mm = gsap.matchMedia();
  const both = (key: 'wide' | 'tall') => () => {
    const svg = section.querySelector<SVGSVGElement>(`[data-how-svg="${key}"]`)!;
    const tl = build(svg, layouts[key]);
    ScrollTrigger.create({
      trigger: section, start: 'top top', end: 'bottom bottom', scrub: 0.6, animation: tl, invalidateOnRefresh: true,
      onUpdate: (self) => {
        setState(Math.min(steps.length - 1, Math.floor(self.progress * steps.length)));
        if (bar) bar.style.scale = `${self.progress} 1`;
      },
    });
    setState(0);
  };
  mm.add('(min-width: 901px) and (prefers-reduced-motion: no-preference)', both('wide'));
  mm.add('(max-width: 900px) and (prefers-reduced-motion: no-preference)', both('tall'));
  return () => mm.revert();
});
