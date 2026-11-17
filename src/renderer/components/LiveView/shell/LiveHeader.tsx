// The live tab's header, one row: the file once (glyph, name, what kind),
// the state chip, one activity sentence, then the right-hand slots (the
// avatar stack, Follow, the view's own controls), the mode area and the
// side-panel toggle. A plain file (the editor, not live) shows no chip; the
// mode control shows live or not; a view with no panel has no toggle. The
// mode area also holds the file tab's "File | Live" switch when the view
// sits in a file tab (LiveEntrySwitch; nothing anywhere else), so the
// header has one place for modes.
// The sentence never shows cut: when it doesn't fit it gives up whole (and
// takes its own row when the tab asks, as the timeline does at 800 px).
// The row never runs past the tab either (editors-notes.md v2, "The narrow
// file bar never wraps"), and the sentence on it is the last thing to go.
// While the row would overflow, or the sentence wouldn't fit whole, it gives
// things up in a fixed order, one step at a time: the file name (the tab has
// it), the avatar stack, the words of Follow and of the buttons after the
// sentence (their marks and short words stay), the file's tile, the long form
// of the sentence (its short form stays), those buttons' short words, then a
// plain "Live" chip.
// The row is the selected tab's colour (color.tab-active), so the tab runs
// into it like a folder tab; its bottom hairline is the edge with the content.

import { useLayoutEffect, useRef, useState } from 'react'
import { cx } from '@/components/ui'
import type { LivePerson } from '@/lib/live-view/shared/people'
import type { LiveTabState } from '@/lib/live-view/shared/state'
import { LiveEntrySwitch } from '../LiveEntrySwitch'
import { PanelGlyph } from './glyphs'
import { LiveAvatar } from './LiveAvatar'
import { StateChip } from './StateChip'

export interface HeaderSentence {
  who: LivePerson | null
  /** The rest of the sentence, after the name. */
  text: string
  /** A short whole form, for a narrow row ("finished."). */
  short?: string
}

/** How many things the row gives up, in order, so it never runs past the tab. */
export const HEADER_GIVE_UP = 7
/** The steps (data-tight): the name, the stack, the words, the tile, the long sentence, the short words,
 *  then a plain "Live" chip (a state worth saying, Reconnecting or Read only, stays). */
export const GIVE_UP = { name: 1, stack: 2, words: 3, tile: 4, sentence: 5, shortWords: 6, liveChip: 7 } as const

function useGiveUp(ref: React.RefObject<HTMLElement | null>, box: React.RefObject<HTMLElement | null>, sameRow: boolean, key: string): number {
  const [tight, setTight] = useState(0)
  // A new width or new content: try the whole row again.
  useLayoutEffect(() => setTight(0), [key])
  // The row runs past the tab, or the sentence on it can't show whole.
  const over = (el: HTMLElement): boolean => {
    if (el.scrollWidth > el.clientWidth + 1) return true
    const b = box.current
    const inner = b?.firstElementChild as HTMLElement | null | undefined
    return !!(sameRow && b && inner && inner.scrollWidth > b.clientWidth + 1)
  }
  const overRef = useRef(over)
  overRef.current = over
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    let w = el.clientWidth
    // The row's own width changed: try it whole again. Something in it grew (a control that
    // measures its own room, a font that loaded): give up the next thing while it overflows.
    const ro = new ResizeObserver(() => {
      if (el.clientWidth !== w) {
        w = el.clientWidth
        setTight(0)
      } else if (overRef.current(el)) setTight((t) => Math.min(HEADER_GIVE_UP, t + 1))
    })
    ro.observe(el)
    for (const child of el.children) ro.observe(child)
    return () => ro.disconnect()
  }, [ref])
  useLayoutEffect(() => {
    const el = ref.current
    if (el && tight < HEADER_GIVE_UP && over(el)) setTight((t) => Math.min(HEADER_GIVE_UP, t + 1))
  })
  return tight
}

/** Hides `ref`'s content whole when it would be cut. */
function useFitsWhole(ref: React.RefObject<HTMLElement | null>, key: string): boolean {
  const [fits, setFits] = useState(true)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const check = (): void => {
      const inner = el.firstElementChild as HTMLElement | null
      setFits(!inner || inner.scrollWidth <= el.clientWidth + 1)
    }
    check()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(check)
    ro.observe(el)
    return () => ro.disconnect()
  }, [ref, key])
  return fits
}

export function LiveHeader({
  fileName,
  fileTitle,
  fileGlyph,
  kindLabel,
  state,
  stateWhy,
  sentence,
  sentenceEnd,
  fitKey,
  sentenceRow = false,
  right,
  mode,
  docked,
  panelOpen = false,
  onTogglePanel,
  panelLabel = 'who’s watching and the changes',
}: {
  fileName: string
  /** The name's tooltip (the editor's path). */
  fileTitle?: string
  fileGlyph?: React.ReactNode
  /** "3D scene", "Timeline": what the file is, quietly. */
  kindLabel?: string
  /** None: no chip (a plain file). */
  state?: LiveTabState
  /** The state's reason (Read only: K2 writes back .otio only); the word alone once the row gives up its short words. */
  stateWhy?: string | null
  sentence: HeaderSentence | null
  /** Right after the sentence (the editors' "Back to my place"); a function gets how much the row gave up. */
  sentenceEnd?: React.ReactNode | ((tight: number) => React.ReactNode)
  /** Changes when the row's controls do (Follow came or went): it tries the whole row again. */
  fitKey?: string
  /** Put the sentence on its own row (narrow windows). */
  sentenceRow?: boolean
  /** The stack, Follow and the view's controls, in that order. */
  right?: React.ReactNode
  /** The view's modes (the editor's Write | Read | Source), after `right`. */
  mode?: React.ReactNode
  /** In the sentence's place while it's set (the editor's formatting toolbar when the margin has no room, w17). */
  docked?: React.ReactNode
  panelOpen?: boolean
  /** No handler: no panel toggle. */
  onTogglePanel?: () => void
  /** What the panel holds, for the toggle's words ("Outline and Live"). */
  panelLabel?: string
}): React.JSX.Element {
  const box = useRef<HTMLDivElement>(null)
  const row = useRef<HTMLDivElement>(null)
  const tight = useGiveUp(row, box, !sentenceRow, `${fitKey ?? ''}|${state ?? ''}${stateWhy ?? ''}|${!!sentenceEnd}|${sentenceRow}|${sentence?.who?.id ?? ''} ${sentence?.text ?? ''}`)
  const words = sentence ? (tight >= GIVE_UP.sentence && sentence.short ? sentence.short : sentence.text) : ''
  const text = sentence ? `${sentence.who?.name ?? ''} ${words}` : ''
  const fits = useFitsWhole(box, `${text}|${sentenceRow}`)
  // The live views' toggle keeps its words ("the side panel"); a view that names its panel says so.
  const onlyName = panelLabel === 'who’s watching and the changes'
  return (
    <div ref={row} data-testid="lv-shell-header" data-tab-header="" data-tight={tight || undefined} data-wrap={sentenceRow ? '' : undefined} className={cx('lv-head flex flex-none items-center gap-x-2.5 border-b border-[var(--color-border)] bg-[var(--color-tab-active)] px-3', sentenceRow ? 'flex-wrap gap-y-1 py-1.5' : 'h-11')}>
      <div className="flex min-w-0 flex-none items-center gap-2">
        {fileGlyph ? (
          <span data-k2-chamfer="control" aria-hidden="true" data-lv-give="4" className="lv-file-glyph inline-flex h-[26px] w-[26px] flex-none items-center justify-center rounded-[var(--radius-field)]">
            {fileGlyph}
          </span>
        ) : null}
        <span title={fileTitle} data-lv-give="1" className="type-name whitespace-nowrap font-semibold text-[var(--color-text-primary)]">
          {fileName}
        </span>
        {kindLabel ? <span className="type-hint whitespace-nowrap text-[var(--color-text-secondary)]">{kindLabel}</span> : null}
      </div>
      {state && !(state === 'live' && tight >= GIVE_UP.liveChip) ? <StateChip state={state} why={stateWhy} short={tight >= GIVE_UP.shortWords} /> : null}
      <div ref={box} aria-live="polite" className={cx('lv-sentence-box flex min-w-0 items-center overflow-hidden', sentenceRow ? 'order-last basis-full' : sentenceEnd ? 'flex-initial' : 'flex-1')}>
        {docked ?? (sentence ? (
          <span className={cx('type-row inline-flex items-center gap-1.5 whitespace-nowrap text-[var(--color-text-primary)]', fits ? '' : 'invisible')} aria-hidden={fits ? undefined : true}>
            {sentence.who ? <LiveAvatar person={sentence.who} size={18} /> : null}
            <span data-testid="lv-sentence">
              {sentence.who ? <b className="font-semibold">{sentence.who.name}</b> : null}
              {sentence.who ? ' ' : ''}
              {words}
            </span>
          </span>
        ) : null)}
      </div>
      {typeof sentenceEnd === 'function' ? sentenceEnd(tight) : sentenceEnd}
      {sentenceRow || sentenceEnd ? <span className="flex-1" /> : null}
      <div className="flex flex-none items-center gap-2">
        {right}
        {mode}
        <LiveEntrySwitch />
        {onTogglePanel ? (
          <button
            type="button"
            data-k2-focus=""
            data-k2-chamfer="control"
            aria-pressed={panelOpen}
            aria-label={panelOpen ? (onlyName ? 'Hide the side panel' : `Hide ${panelLabel}`) : onlyName ? 'Show the side panel' : `Show ${panelLabel}`}
            title={panelOpen ? `Hide ${panelLabel}` : `Show ${panelLabel}`}
            onClick={onTogglePanel}
            data-testid="lv-panel-toggle"
            className="lv-icon-btn no-drag inline-flex h-[30px] w-[30px] cursor-pointer items-center justify-center rounded-[var(--radius-field)] text-[var(--color-text-secondary)]"
          >
            <PanelGlyph size={16} />
          </button>
        ) : null}
      </div>
    </div>
  )
}
