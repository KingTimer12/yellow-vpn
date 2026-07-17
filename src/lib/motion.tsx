// Tiny animation helpers over motion-one's mini bundle (`motion/mini`),
// replacing framer-motion.
//
// `motion/mini` is the ~2.5 KB WAAPI-only `animate()` — no React layer, no
// `variants`, no `AnimatePresence`, and no spring physics. We rebuild only what
// the UI actually uses: staggered entrances, single-element entrances, looping
// animations, and a reduced-motion signal. Tap feedback and simple width
// transitions moved to CSS (Tailwind `active:` + `transition-*`); exit
// animations were dropped (WAAPI can't defer unmount, so elements just leave);
// and the one spring (the status core pop) became an ease-out-back bezier.
//
// Importing from "motion" (root) instead of "motion/mini" would pull the full
// ~35 KB engine and defeat the point — keep these imports on /mini.
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react";
import { animate } from "motion/mini";

type Keyframes = Parameters<typeof animate>[1];
type Options = Parameters<typeof animate>[2];

/** ease-out-back — a slight overshoot, standing in for the old spring pop. */
export const BACK_OUT: [number, number, number, number] = [0.34, 1.56, 0.64, 1];
/** smooth in-out for loops, matching framer's "easeInOut". */
const EASE_IN_OUT: [number, number, number, number] = [0.42, 0, 0.58, 1];
/** the app's signature entrance curve. */
const ENTER_EASE: [number, number, number, number] = [0.16, 1, 0.3, 1];

const REDUCED_QUERY = "(prefers-reduced-motion: reduce)";

/** Reactive `prefers-reduced-motion` flag (matches framer's useReducedMotion). */
export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(
    () => typeof matchMedia !== "undefined" && matchMedia(REDUCED_QUERY).matches,
  );
  useEffect(() => {
    const mq = matchMedia(REDUCED_QUERY);
    const onChange = () => setReduced(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
  return reduced;
}

type StaggerOpts = { y?: number; gap?: number; delay?: number; duration?: number };

/**
 * Attach the returned ref to any container; on mount its `[data-reveal-item]`
 * descendants fade + slide in, staggered (delay = `delay + index * gap`). Works
 * on non-div containers (e.g. a `<ul>` whose children must be `<li>`), which is
 * why it's a hook, not just the <Reveal> component. Items present at mount are
 * hidden synchronously (before paint, no flash); items that mount later — cards
 * revealed after the setup gate — just appear, matching the original single-shot
 * behaviour.
 */
export function useStagger<T extends HTMLElement = HTMLElement>(
  reduce: boolean,
  { y = 14, gap = 0.08, delay = 0.05, duration = 0.4 }: StaggerOpts = {},
): RefObject<T | null> {
  const ref = useRef<T>(null);
  useLayoutEffect(() => {
    const root = ref.current;
    if (!root || reduce) return;
    const items = Array.from(root.querySelectorAll<HTMLElement>("[data-reveal-item]"));
    if (items.length === 0) return;
    for (const el of items) el.style.opacity = "0";
    const controls = items.map((el, i) =>
      animate(
        el,
        { opacity: [0, 1], y: [y, 0] },
        { duration, delay: delay + i * gap, ease: ENTER_EASE },
      ),
    );
    return () => controls.forEach((c) => c.stop());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return ref;
}

type RevealProps = {
  children: ReactNode;
  className?: string;
  reduce?: boolean;
} & StaggerOpts &
  React.HTMLAttributes<HTMLDivElement>;

/** Staggered entrance <div> — thin wrapper over useStagger. */
export function Reveal({
  children,
  className,
  reduce = false,
  y,
  gap,
  delay,
  duration,
  ...rest
}: RevealProps) {
  const ref = useStagger<HTMLDivElement>(reduce, { y, gap, delay, duration });
  return (
    <div ref={ref} className={className} {...rest}>
      {children}
    </div>
  );
}

/**
 * Run a one-shot animation on an element whenever `deps` change (mount, or a
 * `key`/state swap). Returns the ref to attach. No-op under reduced motion.
 */
export function useEnter<T extends HTMLElement = HTMLElement>(
  keyframes: Keyframes,
  options: Options,
  reduce: boolean,
  deps: React.DependencyList = [],
): RefObject<T | null> {
  const ref = useRef<T>(null);
  useEffect(() => {
    if (!ref.current || reduce) return;
    const controls = animate(ref.current, keyframes, options);
    return () => controls.stop();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  return ref;
}

/** Looping animation (repeat forever); no-op under reduced motion. */
export function useLoop<T extends HTMLElement = HTMLElement>(
  keyframes: Keyframes,
  options: Options,
  reduce: boolean,
): RefObject<T | null> {
  const ref = useRef<T>(null);
  useEffect(() => {
    if (!ref.current || reduce) return;
    const controls = animate(ref.current, keyframes, { ...options, repeat: Infinity });
    return () => controls.stop();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reduce]);
  return ref;
}

export { EASE_IN_OUT };
