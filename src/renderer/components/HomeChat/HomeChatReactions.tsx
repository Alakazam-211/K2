// Reactions on a Home chat bubble (prd-home-people-channels-v1 §10, D7):
//   - chips under the bubble: the emoji and its count, a ring when one is
//     yours; a tap toggles yours; hovering (or keyboard focus) shows a tip
//     naming who reacted;
//   - the picker: a small popover in the app's menu shape, six quick emoji
//     and a search over the Unicode set, all by keyboard; Esc closes it.
//
// Look: chips live inside the Thread box, so they take the Thread's tokens
// (`--zen-*`) and its chamfer in Terminal (`data-chamfer`, Thread/chamfer)
// and a pill in Glass (`--radius-pill`). The picker and the tip are
// portalled to the page, outside the Thread box, so the app's own menu
// rules give them their shape (Frost and the box radius in Glass, the cut
// surface in Terminal; float-material).

import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type JSX, type KeyboardEvent } from 'react'
import { createPortal } from 'react-dom'
import type { HomeChatReaction } from '@/lib/home-chat/api'
import { QUICK_REACTIONS, searchEmoji } from '@/lib/home-chat/chat'
import { chamferCutFor, chamferVars } from '@/components/Thread/chamfer'
import { SearchField } from '@/components/ui/SearchField'
import { MENU_LIST, MENU_ROW, floatMaterial } from '@/components/ui/float-material'
import { clientRectToCss, cssViewport } from '@/lib/app-zoom'
import { useBrowserOverlay } from '@/lib/browser-overlay'

/** A chip's height (px): the Thread's small control. */
export const REACTION_CHIP_PX = 22

/** "Ana", "Ana and you"... with You first: who reacted, for the chip's tip. */
export function reactorsLine(names: readonly string[], max = 6): string {
  const you = names.filter((n) => n === 'You')
  const list = [...you, ...names.filter((n) => n !== 'You')]
  if (list.length === 0) return ''
  const shown = list.length > max ? list.slice(0, max - 1) : list
  const rest = list.length - shown.length
  const words = rest > 0 ? [...shown, `${rest} ${rest === 1 ? 'other' : 'others'}`] : shown
  if (words.length === 1) return words[0]
  return `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`
}

/** The tip's words: who reacted with `emoji`, or that it is still asking. */
export function reactionTipText(names: readonly string[] | undefined, emoji: string): string {
  if (names === undefined) return `Finding who reacted with ${emoji}…`
  if (names.length === 0) return `Reacted with ${emoji}`
  return `${reactorsLine(names)} reacted with ${emoji}`
}

const CHIP_BASE: CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 4,
  height: REACTION_CHIP_PX,
  padding: '0 8px 0 6px',
  fontSize: 'var(--zen-type-sm, 0.8em)',
  lineHeight: 1,
  cursor: 'pointer',
  color: 'var(--zen-text)',
  fontVariantNumeric: 'tabular-nums',
}

/** Yours: the readable accent as the ring, a light accent wash inside. */
const MINE_EDGE = 'var(--zen-focus-ring)'
const MINE_FILL = 'color-mix(in srgb, var(--zen-accent) 16%, var(--zen-canvas))'
const CHIP_EDGE = 'var(--zen-control-edge)'
const CHIP_FILL = 'var(--zen-surface)'

function chipStyle(me: boolean, readOnly: boolean): CSSProperties {
  const edge = me ? MINE_EDGE : CHIP_EDGE
  const fill = me ? MINE_FILL : CHIP_FILL
  const border = me ? 1.5 : 1
  return {
    ...CHIP_BASE,
    cursor: readOnly ? 'default' : 'pointer',
    border: `${border}px solid ${edge}`,
    background: fill,
    ...chamferVars({ cut: chamferCutFor(REACTION_CHIP_PX), fill, edge, border }),
  }
}

/** The tip's top (px) for a chip at `chip`: below it, so it never covers
 *  the message the chip belongs to; above it only when the window has no
 *  room below; kept 8px inside the window (exported for tests). */
export function reactionTipTop(chip: { top: number; bottom: number }, h: number, vh: number): number {
  const below = chip.bottom + 6
  if (below + h <= vh - 8) return Math.round(below)
  const above = chip.top - h - 6
  if (above >= 8) return Math.round(above)
  return Math.round(Math.max(8, Math.min(below, vh - h - 8)))
}

/** The tip on a chip: portalled, under it (reactionTipTop), kept 8px inside
 *  the window. Floating chrome: Frost in Glass, a cut chip in Terminal.
 *  Only for the eye: the chip's label says it. */
function ReactionTip({ anchor, text }: { anchor: HTMLElement; text: string }): JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)
  useBrowserOverlay({ active: true, ref, kind: 'popover', watch: pos })
  useLayoutEffect(() => {
    const tip = ref.current
    if (!tip) return
    const a = clientRectToCss(anchor.getBoundingClientRect())
    const { width: vw, height: vh } = cssViewport()
    const w = tip.offsetWidth
    const h = tip.offsetHeight
    const left = Math.max(8, Math.min(a.left + (a.right - a.left) / 2 - w / 2, vw - w - 8))
    setPos({ left: Math.round(left), top: reactionTipTop(a, h, vh) })
  }, [anchor, text])
  return createPortal(
    <div
      ref={ref}
      aria-hidden="true"
      data-testid="home-chat-reaction-tip"
      data-k2-chamfer="chip"
      {...floatMaterial('popover', 'frost')}
      className="pointer-events-none fixed z-[1000] max-w-[280px] rounded-[calc(var(--radius-box)/2)] border border-[var(--color-border)] bg-[var(--color-bg-elevated)] px-2 py-1 text-[11px] leading-[1.35] text-[var(--color-text-primary)] [box-shadow:var(--shadow-2)]"
      style={{ left: pos?.left ?? -9999, top: pos?.top ?? -9999 }}
    >
      {text}
    </div>,
    document.body,
  )
}

/** The chips under one bubble. `whoFor` answers the names for a chip's
 *  tip (asked on hover or focus); `onToggle` flips the viewer's own. */
export function HomeChatReactionChips({
  reactions,
  mine,
  onToggle,
  whoFor,
  disabled = false,
}: {
  reactions: readonly HomeChatReaction[]
  /** The bubble is the viewer's (chips line up on its side). */
  mine: boolean
  onToggle: (emoji: string) => void
  whoFor: (emoji: string) => Promise<string[]>
  disabled?: boolean
}): JSX.Element | null {
  const [who, setWho] = useState<Record<string, string[]>>({})
  const [tip, setTip] = useState<{ emoji: string; el: HTMLElement } | null>(null)
  if (reactions.length === 0) return null
  const ask = (emoji: string, el: HTMLElement): void => {
    setTip({ emoji, el })
    void whoFor(emoji).then(
      (names) => setWho((w) => ({ ...w, [emoji]: names })),
      () => setWho((w) => ({ ...w, [emoji]: [] })),
    )
  }
  const hide = (): void => setTip(null)
  return (
    <div
      data-testid="home-chat-reactions"
      className="flex flex-wrap"
      style={{ gap: 4, alignSelf: mine ? 'flex-end' : 'flex-start', marginTop: 4, padding: '0 4px' }}
    >
      {reactions.map((r) => {
        const names = who[r.emoji]
        const line = names && names.length > 0 ? `: ${reactorsLine(names)}` : r.me ? ', you reacted' : ''
        return (
          <button
            key={r.emoji}
            type="button"
            data-testid="home-chat-reaction"
            data-chamfer=""
            className="k2-focus rounded-[var(--radius-pill)]"
            data-emoji={r.emoji}
            data-me={r.me ? '' : undefined}
            aria-pressed={r.me}
            aria-label={`${r.emoji} ${r.count}${line}`}
            aria-disabled={disabled || undefined}
            onMouseEnter={(e) => ask(r.emoji, e.currentTarget)}
            onMouseLeave={hide}
            onFocus={(e) => ask(r.emoji, e.currentTarget)}
            onBlur={hide}
            onClick={() => {
              if (!disabled) onToggle(r.emoji)
            }}
            style={chipStyle(r.me, disabled)}
          >
            <span aria-hidden="true" style={{ fontSize: '1.1em' }}>
              {r.emoji}
            </span>
            <span data-reaction-count="" style={{ fontWeight: 600 }}>
              {r.count}
            </span>
          </button>
        )
      })}
      {tip && <ReactionTip anchor={tip.el} text={reactionTipText(who[tip.emoji], tip.emoji)} />}
    </div>
  )
}

/** Columns in the picker's grids. */
export const PICKER_COLUMNS = 6
const PICKER_WIDTH = 236
const CELL_CLASS =
  'k2-focus no-drag flex h-8 w-8 cursor-pointer items-center justify-center text-[18px] leading-none outline-none hover:bg-[var(--color-wash-1)] focus-visible:bg-[var(--color-wash-1)]'

type PickerZone = 'quick' | 'search' | 'results'

/** Where a key moves focus in the picker (exported for tests): the quick
 *  row, the search or the results, by index. Null: the key does nothing. */
export function pickerMove(
  at: { zone: PickerZone; index: number },
  key: string,
  counts: { quick: number; results: number },
): { zone: PickerZone; index: number } | null {
  const C = PICKER_COLUMNS
  if (at.zone === 'search') {
    if (key === 'ArrowUp' && counts.quick > 0) return { zone: 'quick', index: 0 }
    if (key === 'ArrowDown' && counts.results > 0) return { zone: 'results', index: 0 }
    return null
  }
  const n = at.zone === 'quick' ? counts.quick : counts.results
  if (key === 'ArrowRight') return { zone: at.zone, index: (at.index + 1) % n }
  if (key === 'ArrowLeft') return { zone: at.zone, index: (at.index - 1 + n) % n }
  if (key === 'Home') return { zone: at.zone, index: 0 }
  if (key === 'End') return { zone: at.zone, index: n - 1 }
  if (at.zone === 'quick') return key === 'ArrowDown' ? { zone: 'search', index: 0 } : null
  if (key === 'ArrowUp') return at.index < C ? { zone: 'search', index: 0 } : { zone: 'results', index: at.index - C }
  if (key === 'ArrowDown') return at.index + C < n ? { zone: 'results', index: at.index + C } : null
  return null
}

/** The picker, anchored to `anchor` (the React button, else the message):
 *  the quick row, then the search, then what it finds. Focus starts in the
 *  search; ↑ goes to the quick row, ↓ to the results, arrows move in them,
 *  Enter picks (in the search: the first match). Esc or a press outside
 *  closes it; Esc and a pick put focus back on `anchor`. */
export function HomeChatReactionPicker({
  anchor,
  align = 'start',
  onPick,
  onClose,
}: {
  anchor: HTMLElement | null
  /** Line up with the anchor's left edge (`start`) or right edge (`end`). */
  align?: 'start' | 'end'
  onPick: (emoji: string) => void
  onClose: () => void
}): JSX.Element {
  const [q, setQ] = useState('')
  const found = useMemo(() => (q.trim() ? searchEmoji(q) : []), [q])
  const ref = useRef<HTMLDivElement | null>(null)
  const searchRef = useRef<HTMLInputElement | null>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)
  const labelId = useId()
  useBrowserOverlay({ active: true, ref, kind: 'popover', watch: pos })

  const close = useCallback(
    (refocus: boolean) => {
      onClose()
      if (refocus && anchor?.isConnected) anchor.focus()
    },
    [onClose, anchor],
  )

  useLayoutEffect(() => {
    const box = ref.current
    if (!box) return
    const { width: vw, height: vh } = cssViewport()
    const w = box.offsetWidth
    const h = box.offsetHeight
    const a = anchor ? clientRectToCss(anchor.getBoundingClientRect()) : { top: vh / 2, bottom: vh / 2, left: vw / 2, right: vw / 2 }
    let top = a.top - h - 6
    if (top < 8) top = Math.min(a.bottom + 6, vh - h - 8)
    const left = Math.max(8, Math.min(align === 'end' ? a.right - w : a.left, vw - w - 8))
    setPos({ left: Math.round(left), top: Math.round(Math.max(8, top)) })
  }, [anchor, align, found.length])

  useEffect(() => {
    searchRef.current?.focus()
    const onDown = (e: MouseEvent): void => {
      if (!(e.target instanceof Node)) return
      if (ref.current?.contains(e.target) || anchor?.contains(e.target)) return
      close(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [close, anchor])

  const pick = (e: string): void => {
    onPick(e)
    close(true)
  }

  const cells = (zone: 'quick' | 'results'): HTMLElement[] =>
    Array.from(ref.current?.querySelectorAll<HTMLElement>(`[data-emoji-zone="${zone}"]`) ?? [])
  const go = (to: { zone: PickerZone; index: number } | null): boolean => {
    if (!to) return false
    if (to.zone === 'search') searchRef.current?.focus()
    else cells(to.zone)[to.index]?.focus()
    return true
  }
  const counts = (): { quick: number; results: number } => ({ quick: QUICK_REACTIONS.length, results: found.length })

  const cell = (e: string, zone: 'quick' | 'results', i: number): JSX.Element => (
    <button
      key={`${zone}-${e}`}
      type="button"
      aria-label={`React ${e}`}
      data-emoji={e}
      data-emoji-zone={zone}
      tabIndex={i === 0 ? 0 : -1}
      data-k2-focus=""
      {...MENU_ROW}
      className={CELL_CLASS}
      onClick={() => pick(e)}
      onKeyDown={(ev: KeyboardEvent<HTMLButtonElement>) => {
        if (go(pickerMove({ zone, index: i }, ev.key, counts()))) ev.preventDefault()
      }}
    >
      {e}
    </button>
  )

  return createPortal(
    <div data-k2-chamfer-float="menu" className="fixed z-[1000]" style={{ left: pos?.left ?? -9999, top: pos?.top ?? -9999 }}>
      <div
        ref={ref}
        role="dialog"
        aria-labelledby={labelId}
        data-testid="home-chat-reaction-picker"
        data-k2-chamfer="surface"
        {...floatMaterial('menu', 'frost')}
        {...MENU_LIST}
        className="flex flex-col gap-1.5 border border-[var(--color-border)] bg-[var(--color-bg)] [box-shadow:var(--shadow-3)]"
        style={{ width: PICKER_WIDTH }}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.preventDefault()
            e.stopPropagation()
            close(true)
          }
        }}
      >
        <span id={labelId} className="sr-only">
          Add a reaction
        </span>
        <div role="group" aria-label="Quick reactions" className="grid grid-cols-6 justify-items-center" data-testid="home-chat-quick-reactions">
          {QUICK_REACTIONS.map((e, i) => cell(e, 'quick', i))}
        </div>
        <SearchField
          ref={searchRef}
          value={q}
          escapeClears={false}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              if (found[0]) pick(found[0])
              return
            }
            if (go(pickerMove({ zone: 'search', index: 0 }, e.key, counts()))) e.preventDefault()
          }}
          placeholder="Search emoji"
          aria-label="Search emoji"
          data-testid="home-chat-emoji-search"
          spellCheck={false}
          autoComplete="off"
          className="w-full"
        />
        {q.trim() !== '' && (
          <div role="group" aria-label="Matching emoji" className="grid max-h-[148px] grid-cols-6 justify-items-center overflow-y-auto" data-testid="home-chat-emoji-results">
            {found.length === 0 ? (
              <span role="status" className="col-span-6 justify-self-start px-1 py-1 text-[11px] text-[var(--color-text-muted)]">
                No emoji match
              </span>
            ) : (
              found.map((e, i) => cell(e, 'results', i))
            )}
          </div>
        )}
      </div>
    </div>,
    document.body,
  )
}
