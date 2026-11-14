// The Thread's "Jump to bottom" pill (Rosson 2026-11-11, matching K2
// Companion's phone pill).
//
// One element, centred over the bottom of the message list, a little above
// the message box:
//   - scrolled up, nothing new: "Jump to bottom ↓" (the quiet surface);
//   - messages from others arrived while scrolled up: "New message ↓", then
//     "N new messages ↓" (the accent treatment);
//   - the person sent while scrolled up (a send doesn't move the list):
//     "Sent ↓" (quiet).
// Where it floats (Rosson 2026-11-14, "match mobile"): THREAD_JUMP_PILL_GAP
// above whatever is topmost on the message box, the activity bar while it
// stands there, else the box itself. The Thread column measures both and
// sets THREAD_DOCK_BOX_VAR (the list's foot to the box's top: a growing
// draft moves the pill at once, as the box grows) and THREAD_DOCK_ABOVE_VAR
// (what stands on the box: the bar, chips). That part moves with the bar's
// own slide, its length and curve (ZenActivityDrawer), so the two travel as
// one; THREAD_DOCK_EASE_VAR is the curve of the move under way. Outside a
// column (no variables) it sits the gap above `bottom`.
//
// A click goes to the bottom (smoothly; a jump under reduced motion). At
// the bottom the pill fades out and the count resets (the list decides,
// `shown` and the words come in as props).
//
// The width animates between the two measured widths of its words (220ms,
// the phone's ease) while `k2-jump-pill--animating` is on; reduced motion:
// the words swap and the width jumps. A polite live region says "N new
// messages" once per count change.
//
// Colours come from the `--zen-*` variables, so the desktop Thread (which
// maps them to the app's Style, THREAD_ZEN_DESKTOP_TOKENS) and any Garden
// that draws it each keep their own look. Glass: Frost (the frost rule
// stops at a Garden root). Terminal: a chamfered small control.

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type JSX } from 'react'
import { ArrowDownIcon } from '@/components/icons/K2Icons'
import { ZEN_DRAWER_EASE_IN, ZEN_DRAWER_MS } from '@/components/Zen/widgets/ZenActivityDrawer'
import { CHAMFER_SCOPE, CHAMFER_CUT, chamferVars } from './chamfer'

/** The pill's own attribute (the CSS hook). */
export const THREAD_JUMP_PILL_ATTR = 'data-thread-jump-pill'
/** On while the width animates between two words. */
export const THREAD_JUMP_PILL_ANIMATING = 'k2-jump-pill--animating'
/** The width animation (K2 Companion's): 220ms. The ease is the Style's
 *  standard ease, which in Glass is the phone's cubic-bezier(0.22, 1, 0.36, 1). */
export const THREAD_JUMP_PILL_WIDTH_MS = 220
/** Space (px) between the pill and what is topmost on the message box (the
 *  activity bar, else the box), or the list's foot. K2 Companion's 8px
 *  (ThreadPane: the pill row's `bottom: 8 + drawerCover`), which is also the
 *  message bar's own top padding. */
export const THREAD_JUMP_PILL_GAP = 8
/** px from the list frame's foot to the message box's top (the column sets it). */
export const THREAD_DOCK_BOX_VAR = '--thread-dock-box'
/** px from the message box's top to the top of what stands on it. */
export const THREAD_DOCK_ABOVE_VAR = '--thread-dock-above'
/** The curve of the move under way: the bar's slide in or out. */
export const THREAD_DOCK_EASE_VAR = '--thread-dock-ease'
/** The pill's own fallback for the box's top (its `bottom` prop). */
const BASE_VAR = '--thread-jump-base'
/** Within this many px of the bottom the list counts as at the bottom. */
export const THREAD_JUMP_BOTTOM_PX = 32

/** What the pill says, and its accessible name (the same words). */
export function threadJumpPillWords(unseen: number, sent: boolean): { text: string; label: string; accent: boolean } {
  if (unseen === 1) return { text: 'New message', label: 'New message, jump to bottom', accent: true }
  if (unseen > 1) return { text: `${unseen} new messages`, label: `${unseen} new messages, jump to bottom`, accent: true }
  if (sent) return { text: 'Sent', label: 'Sent, jump to bottom', accent: false }
  return { text: 'Jump to bottom', label: 'Jump to bottom', accent: false }
}

/** What the live region says for a count (nothing at 0). */
export function threadJumpAnnouncement(unseen: number): string {
  if (unseen <= 0) return ''
  return unseen === 1 ? '1 new message' : `${unseen} new messages`
}

const P = `[${THREAD_JUMP_PILL_ATTR}]`
const FADE = 'var(--motion-duration-fast, 140ms) var(--motion-ease-standard, ease-out)'
const WIDTH = `${THREAD_JUMP_PILL_WIDTH_MS}ms var(--motion-ease-standard, cubic-bezier(0.22, 1, 0.36, 1))`
const RING = 'var(--zen-focus-ring, var(--zen-accent))'
/** Rides the activity bar: its slide's length and curve. */
const RIDE = `translate ${ZEN_DRAWER_MS}ms var(${THREAD_DOCK_EASE_VAR}, ${ZEN_DRAWER_EASE_IN})`
/** Glass with its Frost on (the desktop Thread only; a Garden draws its own):
 *  the quiet pill lets a little of the blurred list through. */
const GLASS = 'html[data-style="glass"]:not([data-frost="tile"]) [data-k2-thread-zen]'

/** The pill's rules (in ThreadZenStyles). Colours are inline: they win
 *  over Glass's frost tint and 0.5px rim, which can miss 4.5:1 / 3:1 over
 *  the messages; Glass's quiet fill is the raised surface at 86% over the
 *  frost blur (solid with reduced transparency). */
export const THREAD_JUMP_PILL_CSS = `
${P} { left: 50%; bottom: calc(var(${THREAD_DOCK_BOX_VAR}, var(${BASE_VAR}, 0px)) + ${THREAD_JUMP_PILL_GAP}px); translate: 0 calc(-1 * var(${THREAD_DOCK_ABOVE_VAR}, 0px)); z-index: 3; display: inline-flex; max-width: calc(100% - 2 * var(--zen-gap, 12px)); align-items: center; margin: 0; padding: 0; border-radius: var(--zen-pill-radius, 999px); font: inherit; font-size: var(--zen-type-sm, 0.8em); line-height: 16px; white-space: nowrap; cursor: pointer; opacity: 0; visibility: hidden; pointer-events: none; transform: translate(-50%, 6px); transition: opacity ${FADE}, transform ${FADE}, ${RIDE}, visibility 0s linear var(--motion-duration-fast, 140ms); }
${P}[data-shown] { opacity: 1; visibility: visible; pointer-events: auto; transform: translate(-50%, 0); transition: opacity ${FADE}, transform ${FADE}, ${RIDE}, visibility 0s; }
${P} [data-jump-clip] { display: block; overflow: hidden; max-width: 100%; }
${P} [data-jump-inner] { display: inline-flex; align-items: center; gap: 6px; width: max-content; padding: 4px 10px 4px 12px; }
${P}.${THREAD_JUMP_PILL_ANIMATING} [data-jump-clip] { transition: width ${WIDTH}; }
${GLASS} ${P}:not([data-accent]) { background: color-mix(in srgb, var(--zen-surface-raised) 86%, transparent) !important; }
${P}:focus-visible { outline: 2px solid ${RING}; outline-offset: 2px; }
${CHAMFER_SCOPE} ${P}[data-chamfer]:focus-visible { outline: none; --ch-edge: ${RING} !important; --ch-bw: 2px !important; }
@media (prefers-reduced-transparency: reduce) {
  ${GLASS} ${P}:not([data-accent]) { background: var(--zen-surface-raised) !important; }
}
@media (prefers-reduced-motion: reduce) {
  ${P}, ${P}[data-shown], ${P}.${THREAD_JUMP_PILL_ANIMATING} [data-jump-clip] { transition: none !important; }
}
`

export interface ThreadJumpPillProps {
  /** Scrolled up from the bottom (and something to jump to). */
  shown: boolean
  /** Messages from others that arrived while scrolled up. */
  unseen: number
  /** The person sent while scrolled up. */
  sent: boolean
  reducedMotion: boolean
  /** Go to the bottom. */
  onJump: () => void
  /** px from the list frame's bottom to the message bar's top, when no
   *  Thread column says where the box is (THREAD_DOCK_BOX_VAR wins). */
  bottom: number
}

/** The fallback for a `transitionend` that never comes (hidden mid-way). */
const ANIMATION_SLACK_MS = 80

export function ThreadJumpPill({ shown, unseen, sent, reducedMotion, onJump, bottom }: ThreadJumpPillProps): JSX.Element {
  const { text, label, accent } = threadJumpPillWords(unseen, sent)
  const innerRef = useRef<HTMLSpanElement | null>(null)
  const [width, setWidth] = useState<number | null>(null)
  const [animating, setAnimating] = useState(false)
  const lastText = useRef(text)
  const shownRef = useRef(shown)
  shownRef.current = shown

  const measure = useCallback((): number | null => {
    const el = innerRef.current
    if (!el) return null
    return Math.ceil(el.getBoundingClientRect().width)
  }, [])

  // New words: measure them before paint; animate from the old width while
  // the pill is on screen (never under reduced motion).
  useLayoutEffect(() => {
    const next = measure()
    if (lastText.current !== text) {
      lastText.current = text
      if (!reducedMotion && shownRef.current && width !== null) setAnimating(true)
    }
    if (next !== null) setWidth(next)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text, measure, reducedMotion])

  // The font changed size (the Thread's text size): the new width, at once.
  useEffect(() => {
    const el = innerRef.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => {
      const next = measure()
      if (next !== null) setWidth((w) => (w === next ? w : next))
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [measure])

  useEffect(() => {
    if (!animating) return
    const id = setTimeout(() => setAnimating(false), THREAD_JUMP_PILL_WIDTH_MS + ANIMATION_SLACK_MS)
    return () => clearTimeout(id)
  }, [animating, width])

  useEffect(() => {
    if (reducedMotion) setAnimating(false)
  }, [reducedMotion])

  const fill = accent ? 'var(--zen-accent-fill, var(--zen-accent))' : 'var(--zen-surface-raised)'
  const edge = accent ? fill : 'var(--zen-control-edge, var(--zen-border))'
  const style: CSSProperties = {
    // Inline: Terminal's chamfer rule makes a cut element `relative`.
    position: 'absolute',
    ...({ [BASE_VAR]: `${bottom}px` } as CSSProperties),
    background: fill,
    border: `1px solid ${edge}`,
    color: accent ? 'var(--zen-accent-text)' : 'var(--zen-text)',
    // Terminal Style: a small control's cut, its edge drawn by the layers.
    ...chamferVars({ cut: CHAMFER_CUT.button, fill, edge, border: 1 }),
  }
  const announce = threadJumpAnnouncement(unseen)

  return (
    <>
      <button
        type="button"
        {...{ [THREAD_JUMP_PILL_ATTR]: '' }}
        data-testid="thread-jump-pill"
        data-shown={shown ? '' : undefined}
        data-accent={accent ? '' : undefined}
        data-material="frost"
        data-frost-shape="pill"
        data-chamfer=""
        className={animating ? THREAD_JUMP_PILL_ANIMATING : undefined}
        aria-label={label}
        aria-hidden={shown ? undefined : true}
        tabIndex={shown ? 0 : -1}
        style={style}
        onClick={onJump}
      >
        <span
          data-jump-clip=""
          style={width !== null ? { width } : undefined}
          onTransitionEnd={(e) => {
            if (e.propertyName === 'width') setAnimating(false)
          }}
        >
          <span ref={innerRef} data-jump-inner="">
            <span data-jump-text="">{text}</span>
            <ArrowDownIcon size={14} />
          </span>
        </span>
      </button>
      <span className="sr-only" aria-live="polite" aria-atomic="true" data-testid="thread-jump-live">
        {announce}
      </span>
    </>
  )
}
