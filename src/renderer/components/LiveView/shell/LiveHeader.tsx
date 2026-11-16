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
// file bar never wraps"): while it would, it gives things up in a fixed
// order, one step at a time: the file name (the tab has it), the avatar
// stack, Follow's words (its mark and "Stop" stay), then the file's tile.

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
}

/** How many things the row gives up, in order, so it never runs past the tab. */
export const HEADER_GIVE_UP = 4

function useGiveUp(ref: React.RefObject<HTMLElement | null>, key: string): number {
  const [tight, setTight] = useState(0)
  // A new width or new content: try the whole row again.
  useLayoutEffect(() => setTight(0), [key])
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
      } else if (el.scrollWidth > el.clientWidth + 1) setTight((t) => Math.min(HEADER_GIVE_UP, t + 1))
    })
    ro.observe(el)
    for (const child of el.children) ro.observe(child)
    return () => ro.disconnect()
  }, [ref])
  useLayoutEffect(() => {
    const el = ref.current
    if (el && tight < HEADER_GIVE_UP && el.scrollWidth > el.clientWidth + 1) setTight((t) => Math.min(HEADER_GIVE_UP, t + 1))
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
  sentence,
  sentenceEnd,
  fitKey,
  sentenceRow = false,
  right,
  mode,
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
  sentence: HeaderSentence | null
  /** Right after the sentence, kept when the sentence can't fit (the editors' "Back to my place"). */
  sentenceEnd?: React.ReactNode
  /** Changes when the row's controls do (Follow came or went): it tries the whole row again. */
  fitKey?: string
  /** Put the sentence on its own row (narrow windows). */
  sentenceRow?: boolean
  /** The stack, Follow and the view's controls, in that order. */
  right?: React.ReactNode
  /** The view's modes (the editor's Write | Read | Source), after `right`. */
  mode?: React.ReactNode
  panelOpen?: boolean
  /** No handler: no panel toggle. */
  onTogglePanel?: () => void
  /** What the panel holds, for the toggle's words ("Outline and Live"). */
  panelLabel?: string
}): React.JSX.Element {
  const box = useRef<HTMLDivElement>(null)
  const row = useRef<HTMLDivElement>(null)
  const text = sentence ? `${sentence.who?.name ?? ''} ${sentence.text}` : ''
  const fits = useFitsWhole(box, `${text}|${sentenceRow}`)
  const tight = useGiveUp(row, `${fitKey ?? ''}|${state ?? ''}|${!!sentenceEnd}|${sentenceRow}`)
  // The live views' toggle keeps its words ("the side panel"); a view that names its panel says so.
  const onlyName = panelLabel === 'who’s watching and the changes'
  return (
    <div ref={row} data-testid="lv-shell-header" data-tight={tight || undefined} data-wrap={sentenceRow ? '' : undefined} className={cx('lv-head flex flex-none items-center gap-x-2.5 border-b border-[var(--color-border)] bg-[var(--color-bg)] px-3', sentenceRow ? 'flex-wrap gap-y-1 py-1.5' : 'h-11')}>
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
      {state ? <StateChip state={state} /> : null}
      <div ref={box} aria-live="polite" className={cx('lv-sentence-box flex min-w-0 items-center overflow-hidden', sentenceRow ? 'order-last basis-full' : sentenceEnd ? 'flex-initial' : 'flex-1')}>
        {sentence ? (
          <span className={cx('type-row inline-flex items-center gap-1.5 whitespace-nowrap text-[var(--color-text-primary)]', fits ? '' : 'invisible')} aria-hidden={fits ? undefined : true}>
            {sentence.who ? <LiveAvatar person={sentence.who} size={18} /> : null}
            <span data-testid="lv-sentence">
              {sentence.who ? <b className="font-semibold">{sentence.who.name}</b> : null}
              {sentence.who ? ' ' : ''}
              {sentence.text}
            </span>
          </span>
        ) : null}
      </div>
      {sentenceEnd}
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
