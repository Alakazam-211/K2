// The desktop Thread's reply bar: what the next send replies to, standing
// on the message box (Rosson 2026-11-15: "the reply object that floats
// above the activity ear or message the agent area is kinda bad too. It is
// wider than the activity ear and it's completely transparent").
//
// - Where: a row of the message bar, on the ear's own edges
//   (THREAD_EAR_INSET_PX in from the box, as the activity bar and the stash),
//   THREAD_REPLY_BAR_GAP over the ear (the Jump to bottom pill's gap). In the
//   bar's flow, so it comes down with the ear when the ear slides away, and
//   the pill floats over it (threadDock counts the bar's rows).
// - What: "Replying to <name>" (or "…part of…"), the quoted message's first
//   line in whole words (a tail that doesn't fit goes as a whole phrase,
//   then "…"), and ×. × or Esc cancels (Esc from the box: TerminalComposeBar).
// - Glass: the Jump to bottom pill's glass (the shared Frost, the pill's
//   scrim, rim and ink, `--k2-jump-*` in globals.css), so the words read
//   at 4.5:1 over any message; every word in the ink, since the muted grey
//   misses 4.5:1 over your blue bubble. Reduced transparency: the Frost's
//   solid tile.
// - Terminal: a solid raised surface, the control edge, cut corners.
//
// A Garden's box (ZenCompose) and Home chat keep ThreadReplyChip.

import { useLayoutEffect, useRef, useState, type CSSProperties, type JSX } from 'react'
import { threadReplyFirstLine, threadReplyQuote, type ThreadReplyTarget } from '@/lib/thread-reply'
import { ReplyGlyph } from './ThreadMessageMenu'
import { CHAMFER_CUT, CHAMFER_SCOPE, chamferVars } from './chamfer'

/** The ear's side inset on the desktop Thread's box (px): the activity
 *  bar's and the stash's row (TerminalComposeBar), and this bar's. */
export const THREAD_EAR_INSET_PX = 14
/** Space (px) between the bar and the ear under it: the pill's. */
export const THREAD_REPLY_BAR_GAP = 8

const B = '[data-thread-reply-bar]'
const GLASS = 'html[data-style="glass"] [data-k2-thread-zen]'
const FROST = `html[data-style="glass"]:not([data-frost="tile"]) [data-k2-thread-zen] ${B}[data-material="frost"]`
const RING = 'var(--zen-focus-ring, var(--zen-accent))'

/** The bar's rules. Terminal paints through its chamfer layers; Glass takes
 *  the shared Frost and the pill's tokens. */
export const THREAD_REPLY_BAR_CSS = `
${B} { display: flex; align-items: center; gap: 6px; min-width: 0; min-height: 28px; box-sizing: border-box; margin: 0; padding: 0 4px 0 10px; background: var(--zen-surface-raised); border: 1px solid var(--zen-control-edge, var(--zen-border)); border-radius: var(--zen-pill-radius, 999px); color: var(--zen-text); font-size: var(--zen-type-sm, 0.8em); line-height: 16px; white-space: nowrap; }
${B} [data-reply-chip-who] { flex: 0 1 auto; min-width: 0; max-width: 55%; overflow: hidden; text-overflow: ellipsis; font-weight: 600; color: inherit; }
${B} [data-reply-chip-who]::after { content: ":"; }
${B} [data-reply-chip-text] { flex: 1 1 0; min-width: 0; overflow: hidden; color: inherit; }
${B} [data-reply-cancel] { display: inline-flex; flex: none; align-items: center; justify-content: center; width: 24px; height: 24px; margin: 0; padding: 0; border: none; border-radius: 999px; background: transparent; color: inherit; font: inherit; font-size: 15px; line-height: 1; cursor: pointer; }
${B} [data-reply-cancel]:hover { background: color-mix(in srgb, currentColor 14%, transparent); }
${B} [data-reply-cancel]:focus-visible { outline: 2px solid ${RING}; outline-offset: 1px; }
${CHAMFER_SCOPE} ${B} [data-reply-cancel] { border-radius: 0; }
${GLASS} ${B} { min-height: var(--k2-jump-h); padding: 0 4px 0 var(--k2-jump-pad); color: var(--k2-jump-ink); font-size: var(--k2-jump-type); font-weight: 400; }
${GLASS} ${B} :is([data-reply-chip-who], [data-reply-chip-text], [data-reply-cancel]) { color: var(--k2-jump-ink); }
${FROST} { background: linear-gradient(var(--k2-jump-tint) 0 0), var(--k2-jump-scrim); border-color: var(--k2-jump-edge); }
`

/** `line` cut to whole words that fit `room` px (measured by `width`), then
 *  "…"; just "…" when not even the first word fits. `room` 0: not measured
 *  yet, the whole line. */
export function fitReplyWords(line: string, room: number, width: (s: string) => number): string {
  if (room <= 0 || width(line) <= room) return line
  const words = line.split(/\s+/).filter(Boolean)
  for (let n = words.length - 1; n > 0; n--) {
    const s = `${words.slice(0, n).join(' ').replace(/[\s,;:.–—-]+$/, '')}…`
    if (width(s) <= room) return s
  }
  return '…'
}

/** Text width in `el`'s own font (a canvas; null where there is none). */
function textWidth(el: HTMLElement): ((s: string) => number) | null {
  const ctx = typeof document !== 'undefined' ? document.createElement('canvas').getContext?.('2d') : null
  if (!ctx) return null
  const cs = getComputedStyle(el)
  ctx.font = cs.font || `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`
  return (s) => ctx.measureText(s).width
}

/** The first line, fitted to its slot in whole words (and refitted when the
 *  slot changes size). */
function useFittedLine(line: string): { ref: React.RefObject<HTMLSpanElement | null>; text: string } {
  const ref = useRef<HTMLSpanElement | null>(null)
  const [room, setRoom] = useState(0)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const read = (): void => setRoom(Math.floor(el.clientWidth))
    read()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(read)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  const el = ref.current
  const width = el && room > 0 ? textWidth(el) : null
  return { ref, text: width ? fitReplyWords(line, room, width) : line }
}

export function ThreadReplyBar({ target, onCancel }: { target: ThreadReplyTarget; onCancel: () => void }): JSX.Element {
  const line = threadReplyFirstLine(threadReplyQuote(target))
  const who = `${target.part ? 'Replying to part of' : 'Replying to'} ${target.from}`
  const fitted = useFittedLine(line)
  const style = chamferVars({
    cut: CHAMFER_CUT.button,
    fill: 'var(--zen-surface-raised)',
    edge: 'var(--zen-control-edge, var(--zen-border))',
    border: 1,
  }) as CSSProperties
  return (
    <div data-thread-reply-row="" style={{ margin: `0 ${THREAD_EAR_INSET_PX}px ${THREAD_REPLY_BAR_GAP}px`, minWidth: 0 }}>
      <style data-thread-reply-bar-styles="">{THREAD_REPLY_BAR_CSS}</style>
      <div
        data-testid="thread-reply-chip"
        data-zen-reply-chip=""
        data-thread-reply-bar=""
        data-reply-part={target.part ? '' : undefined}
        data-material="frost"
        data-frost-shape="pill"
        data-chamfer=""
        role="note"
        aria-label={`${who}: ${line}`}
        title={`${who}: ${line}\nEsc cancels.`}
        style={style}
        onKeyDown={(e) => {
          if (e.key !== 'Escape' || e.nativeEvent.isComposing) return
          e.preventDefault()
          e.stopPropagation()
          onCancel()
        }}
      >
        <ReplyGlyph size={14} />
        <span data-reply-chip-who="">{who}</span>
        <span data-reply-chip-text="" ref={fitted.ref}>
          {fitted.text}
        </span>
        <button
          type="button"
          aria-label="Cancel the reply"
          title="Cancel the reply (Esc)"
          data-testid="thread-reply-cancel"
          data-reply-cancel=""
          onClick={(e) => {
            e.stopPropagation()
            onCancel()
          }}
        >
          ×
        </button>
      </div>
    </div>
  )
}
