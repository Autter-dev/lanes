// Gateway flow, in cause-and-effect order: the lanes draw in and reach the gateway first, the
// gateway lights, and only then do Autter's checks branch out. After that, work keeps flowing the
// same way round: every cycle the lanes deliver into the gateway before anything leaves it.
// Plays once when the figure comes into view; the loop runs only while it is on screen.
import { gsap } from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';
import { DrawSVGPlugin } from 'gsap/DrawSVGPlugin';
import { MotionPathPlugin } from 'gsap/MotionPathPlugin';
import { onPage, prefersReducedMotion } from '../engine';

gsap.registerPlugin(ScrollTrigger, DrawSVGPlugin, MotionPathPlugin);

const IN = 1.3;   // seconds for work to travel from a lane to the gateway
const OUT = 1.0;  // seconds from the gateway to a check

export const startGateway = () => onPage(() => {
  document.querySelectorAll<HTMLElement>('[data-gwflow]').forEach((fig) => {
    const svg = fig.querySelector('svg');
    if (!svg || prefersReducedMotion()) return;
    const inPaths = [...svg.querySelectorAll<SVGPathElement>('[id^="gw-in-"]')];
    const outPaths = [...svg.querySelectorAll<SVGPathElement>('[id^="gw-out-"]')];
    const boundary = svg.querySelector('.s-boundary');
    const dots = [...svg.querySelectorAll<SVGCircleElement>('[data-flow-dot]')];
    const inDots = dots.filter((d) => d.dataset.flowDot!.startsWith('gw-in-'));
    const outDots = dots.filter((d) => d.dataset.flowDot!.startsWith('gw-out-'));
    const node = svg.querySelector('[data-gw-node]');
    const core = svg.querySelector('[data-gw-core]');
    const outLabels = outPaths.map((p) => p.parentElement!.querySelectorAll('text'));

    gsap.set(dots, { autoAlpha: 0 });

    // 1. Build-up, in order: lanes → gateway → checks.
    const intro = gsap.timeline({ paused: true, defaults: { ease: 'power2.inOut' } })
      .from(boundary, { drawSVG: '0%', duration: 0.6 }, 0)
      .from(inPaths, { drawSVG: '0%', duration: 0.9, stagger: 0.12 }, 0)
      .from([node, core], { scale: 0, transformOrigin: '50% 50%', duration: 0.45, ease: 'expo.out' }, 1.05)
      .from(outPaths, { drawSVG: '0%', duration: 0.6, stagger: 0.08 }, 1.35)
      .from(outLabels, { autoAlpha: 0, x: -8, duration: 0.4, stagger: 0.08, ease: 'power3.out' }, 1.6);

    // 2. The steady flow: three pieces of work arrive, the gateway pulses, four checks go out.
    const mp = (path: SVGPathElement) => ({ path, align: path, alignOrigin: [0.5, 0.5] as [number, number], start: 0, end: 1 });
    const flow = gsap.timeline({ repeat: -1, paused: true });
    inDots.forEach((dot, i) => {
      const path = svg.querySelector(`#${dot.dataset.flowDot}`) as SVGPathElement;
      const at = i * 0.18;
      flow.set(dot, { autoAlpha: 1 }, at)
        .to(dot, { motionPath: mp(path), duration: IN, ease: 'power1.inOut', immediateRender: true }, at)
        .set(dot, { autoAlpha: 0 }, at + IN);
    });
    const arrived = (inDots.length - 1) * 0.18 + IN;
    flow.to(core, { scale: 1.6, transformOrigin: '50% 50%', duration: 0.2, yoyo: true, repeat: 1, ease: 'power2.out' }, arrived);
    outDots.forEach((dot, i) => {
      const path = svg.querySelector(`#${dot.dataset.flowDot}`) as SVGPathElement;
      const at = arrived + 0.3 + i * 0.1;
      flow.set(dot, { autoAlpha: 1 }, at)
        .to(dot, { motionPath: mp(path), duration: OUT, ease: 'power1.out', immediateRender: true }, at)
        .set(dot, { autoAlpha: 0 }, at + OUT);
    });
    flow.to({}, { duration: 0.6 });

    let started = false;
    ScrollTrigger.create({
      trigger: fig, start: 'top 70%', end: 'bottom top',
      onToggle: (self) => {
        if (!self.isActive) return void flow.pause();
        if (!started) { started = true; intro.play().then(() => flow.play()); }
        else if (intro.progress() === 1) flow.play();
      },
    });
  });
});
