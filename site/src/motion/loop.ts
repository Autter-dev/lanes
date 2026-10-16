// How Lanes works with Autter: while the stage is pinned, scroll drives one agent along the lane
// through write, check, ship and fix, lights each station as it arrives, then draws the return
// line from fix back to write.
import { gsap } from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';
import { DrawSVGPlugin } from 'gsap/DrawSVGPlugin';
import { onPage } from '../engine';

gsap.registerPlugin(ScrollTrigger, DrawSVGPlugin);

export const startLoop = () => onPage(() => {
  const mm = gsap.matchMedia();
  document.querySelectorAll<HTMLElement>('[data-loop]').forEach((section) => {
    const track = section.querySelector<HTMLElement>('.loop__track')!;
    const agent = section.querySelector<HTMLElement>('[data-loop-agent]')!;
    const bar = section.querySelector<HTMLElement>('[data-loop-progress]')!;
    const stations = [...section.querySelectorAll<HTMLElement>('[data-station]')];
    const ret = section.querySelector('[data-loop-return]');
    const back = section.querySelector('[data-loop-back]');

    const run = (vertical: boolean) => () => {
      // Station centres along the track, as fractions: 1/8, 3/8, 5/8, 7/8 across, or each row down.
      const stops = () => stations.map((s) => {
        const r = s.getBoundingClientRect(), t = track.getBoundingClientRect();
        return vertical ? (r.top - t.top + 14) / t.height : (r.left - t.left + r.width / 2) / t.width;
      });
      let at = stops();
      const prop = vertical ? 'y' : 'x';
      const size = () => (vertical ? track.clientHeight : track.clientWidth);
      const place = (f: number) => gsap.set(agent, { [prop]: f * size() });
      const fill = (f: number) => (bar.style.scale = vertical ? `1 ${f}` : `${f} 1`);

      gsap.set(ret, { drawSVG: '0%' });
      gsap.set(back, { autoAlpha: 0 });
      place(at[0]);
      fill(at[0]);
      ScrollTrigger.create({
        trigger: section, start: 'top top', end: 'bottom bottom', scrub: true, invalidateOnRefresh: true,
        onRefresh: () => { at = stops(); },
        onUpdate: ({ progress: p }) => {
          // The agent travels for the first 80% of the scroll; the last 20% draws the way back.
          const travel = Math.min(1, p / 0.8);
          const f = at[0] + (at[at.length - 1] - at[0]) * travel;
          place(f);
          fill(f);
          stations.forEach((s, i) => s.toggleAttribute('data-on', f >= at[i] - 0.01));
          const back01 = Math.max(0, (p - 0.8) / 0.2);
          gsap.set(ret, { drawSVG: `${back01 * 100}%` });
          gsap.set(back, { autoAlpha: back01 > 0.6 ? 1 : 0 });
        },
      });
    };
    mm.add('(min-width: 901px) and (prefers-reduced-motion: no-preference)', run(false));
    mm.add('(max-width: 900px) and (prefers-reduced-motion: no-preference)', run(true));
  });
  return () => mm.revert();
});
