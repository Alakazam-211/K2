// The Style's slow motion, read live from its tokens, for motion driven
// from script (Web Animations): the setup list's fold and the sidebar's
// floor move together on it. Terminal: 300ms linear; Glass: 320ms
// cubic-bezier(0.22, 1, 0.36, 1). The medium motion (a row moving to its
// new place): Terminal 150ms linear; Glass 180ms on the same curve.

/** Used when the Style's `--motion-duration-slow` can't be read. */
export const SLOW_MOTION_FALLBACK_MS = 300

/** A CSS time (`320ms`, `0.3s`) in ms; null when it isn't one. */
export function cssTimeMs(raw: string): number | null {
  const m = /^\s*([\d.]+)\s*(ms|s)\s*$/.exec(raw)
  if (!m) return null
  const n = Number(m[1])
  if (!Number.isFinite(n)) return null
  return m[2] === 's' ? n * 1000 : n
}

/** Used when the Style's `--motion-duration-medium` can't be read. */
export const MEDIUM_MOTION_FALLBACK_MS = 180

function motionTiming(token: string, fallbackMs: number): { duration: number; easing: string } {
  const css = getComputedStyle(document.documentElement)
  const duration = cssTimeMs(css.getPropertyValue(token)) ?? fallbackMs
  const easing = css.getPropertyValue('--motion-ease-standard').trim() || 'ease-out'
  return { duration, easing }
}

/** `--motion-duration-slow` and `--motion-ease-standard`, as WAAPI timing. */
export function slowMotionTiming(): { duration: number; easing: string } {
  return motionTiming('--motion-duration-slow', SLOW_MOTION_FALLBACK_MS)
}

/** `--motion-duration-medium` and `--motion-ease-standard`, as WAAPI timing. */
export function mediumMotionTiming(): { duration: number; easing: string } {
  return motionTiming('--motion-duration-medium', MEDIUM_MOTION_FALLBACK_MS)
}
