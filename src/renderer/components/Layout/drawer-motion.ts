// Drawer open/close animation settings (0.46.0 UI). Used by
// `AnimatedDrawer.tsx` in every room shell (Agents/Home Layout, Focus).

export const DRAWER_ANIM_MS = 250

/** Ease-out: fast start, gentle settle. */
export const DRAWER_ANIM_EASE = 'cubic-bezier(0.2, 0, 0, 1)'

export const DRAWER_TRANSITION = `width ${DRAWER_ANIM_MS}ms ${DRAWER_ANIM_EASE}`

/** False (instant open/close) under prefers-reduced-motion, and where
 *  there is no matchMedia at all (non-browser test environments). */
export function drawerMotionOn(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false
  return !window.matchMedia('(prefers-reduced-motion: reduce)').matches
}
