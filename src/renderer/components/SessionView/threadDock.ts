// Where the Thread's Jump to bottom pill floats (Rosson 2026-11-14: "If you
// look on mobile, it floats just above the activity bar and when the
// activity bar is collapsed it also moves down to be just above the message
// the agent text area"). The column measures its message bar and hands the
// pill two numbers as CSS variables on its root (ThreadJumpPill reads them):
//   - box: the list's foot to the message box's top. A growing draft grows
//     the box, and the pill rises with it at once;
//   - above: the box's top to the top of what stands on it (the activity
//     bar, the stash, chips). The pill moves this part with the bar's own
//     slide, so the two travel as one.
// A bar sliding out (its `data-phase="out"`) no longer counts, though it
// keeps its room until the slide ends: the pill starts down with it.

import { THREAD_DOCK_ABOVE_VAR, THREAD_DOCK_BOX_VAR, THREAD_DOCK_EASE_VAR } from '@/components/Thread/ThreadJumpPill'
import { FOOTER_FADE_TARGETS } from '@/components/Thread/ThreadZenKit'
import { ZEN_DRAWER_EASE_IN, ZEN_DRAWER_EASE_OUT } from '@/components/Zen/widgets/ZenActivityDrawer'

export interface ThreadDock {
  /** px from the slot's foot to the message box's top (the slot's height
   *  when it has no box). */
  box: number
  /** px from the box's top to the top of what stands on it (0: nothing). */
  above: number
}

const BOX = '[data-zen-compose-box]'
const CLIP = '[data-zen-drawer-clip]'
/** The activity bar sliding out. */
const LEAVING = '[data-zen-drawer][data-phase="out"]'

/** What stands on the box: the footer fade's targets but the box, and the
 *  bar's own rows (a "waiting" or stash note over the box, the reply chip). */
const STANDING = [...FOOTER_FADE_TARGETS.filter((sel) => sel !== BOX), '[data-compose-bar] > *'].join(', ')

/** The dock of the message bar in `slot` (the column's compose slot). */
export function measureThreadDock(slot: HTMLElement): ThreadDock {
  const s = slot.getBoundingClientRect()
  const boxEl = slot.querySelector(BOX)
  if (!boxEl) return { box: Math.max(0, Math.round(s.height)), above: 0 }
  const boxTop = boxEl.getBoundingClientRect().top
  // A leaving bar's room (still laid out until its slide ends) and its top.
  let leaving = 0
  let leavingTop = Infinity
  for (const clip of Array.from(slot.querySelectorAll(CLIP))) {
    if (!clip.querySelector(LEAVING)) continue
    const r = clip.getBoundingClientRect()
    if (r.height <= 0) continue
    leaving = Math.max(leaving, r.height)
    leavingTop = Math.min(leavingTop, r.top)
  }
  let top = boxTop
  for (const node of Array.from(slot.querySelectorAll(STANDING))) {
    // The leaving bar, and a row that holds it (its room goes with it).
    if (node.querySelector(LEAVING) || node.matches(LEAVING)) continue
    if (!(node instanceof HTMLElement) || !inFlow(node)) continue
    const r = node.getBoundingClientRect()
    if (r.height <= 0) continue
    // What sits over a leaving bar comes down by its room when it goes.
    top = Math.min(top, leaving > 0 && r.bottom <= leavingTop + 1 ? r.top + leaving : r.top)
  }
  return { box: Math.max(0, Math.round(s.bottom - boxTop)), above: Math.max(0, Math.round(boxTop - top)) }
}

/** Laid out in the bar (a menu floating over it isn't). */
function inFlow(el: HTMLElement): boolean {
  const pos = getComputedStyle(el).position
  return pos !== 'absolute' && pos !== 'fixed'
}

/** Puts `dock` on the column's root. The move's curve is the bar's: its
 *  slide in when what stands on the box grows, its slide out when it
 *  shrinks (set with the new value, so this move takes it). Null clears. */
export function writeThreadDock(root: HTMLElement, dock: ThreadDock | null): void {
  const st = root.style
  if (!dock) {
    st.removeProperty(THREAD_DOCK_BOX_VAR)
    st.removeProperty(THREAD_DOCK_ABOVE_VAR)
    st.removeProperty(THREAD_DOCK_EASE_VAR)
    return
  }
  const before = Number.parseFloat(st.getPropertyValue(THREAD_DOCK_ABOVE_VAR))
  const above = `${dock.above}px`
  if (Number.isFinite(before) && before !== dock.above) {
    st.setProperty(THREAD_DOCK_EASE_VAR, dock.above > before ? ZEN_DRAWER_EASE_IN : ZEN_DRAWER_EASE_OUT)
  }
  st.setProperty(THREAD_DOCK_BOX_VAR, `${dock.box}px`)
  st.setProperty(THREAD_DOCK_ABOVE_VAR, above)
}
