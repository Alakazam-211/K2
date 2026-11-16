// The editors' find bar (build-editors.md E3; editors-notes.md §2 Find, v2
// "the find bar docks", v3.2): docked under the file bar, full width, pushing
// the text down. The field, the count ("3 of 12", "No matches"), Previous and
// Next (off but focusable when nothing matches, with the reason), Aa, W and
// .* as toggles, the Replace toggle and Close; Replace opens a second row
// (the field, Replace, Replace all). One bar for Write, Read, Source, code
// and a live doc: it drives CodeMirror's search on whichever view is mounted
// (lib/editor/find). Enter and Shift+Enter step; Esc closes and puts the
// keyboard back in the text.

import { useEffect, useLayoutEffect, useRef } from 'react'
import type { EditorView } from '@codemirror/view'
import { cx } from '@/components/ui'
import { countWords, findCount, findStep, firstFrom, replaceAllSpec, replaceOneSpec, selectMatch, type FindSpec } from '@/lib/editor/find/find'
import { hideMatches, showMatches } from '@/lib/editor/find/find-view'
import { ToolGlyph } from './glyphs'

export interface FindBarProps {
  view: EditorView | null
  /** The view's update count: the bar redraws with it. */
  version: number
  fileName: string
  spec: FindSpec
  setSpec: (s: FindSpec) => void
  replaceOpen: boolean
  setReplaceOpen: (open: boolean) => void
  /** Why this file can't take a replace, or null. */
  readOnly: string | null
  /** Changes each time the bar is asked for (⌘F again): the field takes the keyboard. */
  focusKey: number
  /** Replace was asked for (⌥⌘F): its field takes the keyboard. */
  focusReplace: boolean
  onClose: () => void
}

function Toggle({ label, title, on, onClick, children }: { label: string; title?: string; on: boolean; onClick: () => void; children: React.ReactNode }): React.JSX.Element {
  return (
    <button type="button" data-k2-focus="" data-k2-chamfer="control" aria-label={label} title={title ?? label} aria-pressed={on} onClick={onClick} className="ed-find-tgl no-drag">
      {children}
    </button>
  )
}

export function FindBar(p: FindBarProps): React.JSX.Element {
  const { view, spec, setSpec } = p
  const field = useRef<HTMLInputElement>(null)
  const replaceField = useRef<HTMLInputElement>(null)
  // Where typing in the field searches from: the selection when the bar opened.
  const anchor = useRef<number | null>(null)
  const shown = useRef<EditorView | null>(null)

  useLayoutEffect(() => {
    anchor.current = view ? view.state.selection.main.from : null
    const el = p.focusReplace ? replaceField.current : field.current
    el?.focus()
    el?.select()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p.focusKey])

  // The query on the view: its matches marked, the first one from where you started selected.
  useEffect(() => {
    if (!view) return
    showMatches(view, spec)
    shown.current = view
    if (!spec.search) return
    const m = firstFrom(view.state, spec, anchor.current ?? view.state.selection.main.from)
    const sel = view.state.selection.main
    if (m && (m.from !== sel.from || m.to !== sel.to)) view.dispatch(selectMatch(m))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, spec.search, spec.caseSensitive, spec.wholeWord, spec.regexp])

  // The query's replacement goes on the view's query too (CodeMirror keeps one).
  useEffect(() => {
    if (view && shown.current === view) showMatches(view, spec)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [spec.replace])

  // Leaving: the matches go.
  useEffect(() => () => void (shown.current && hideMatches(shown.current)), [])

  const count = view ? findCount(view.state, spec) : { total: 0, current: 0, invalid: false, capped: false, empty: true }
  const words = countWords(count)
  const none = count.total === 0
  const offWhy = count.invalid ? 'Not a valid pattern' : 'No matches'
  const canReplace = p.readOnly === null && !!view && !view.state.readOnly
  const roWhy = p.readOnly ?? 'This file can’t be edited now'

  const step = (dir: 1 | -1): void => {
    if (!view) return
    const m = findStep(view.state, spec, dir)
    if (m) view.dispatch(selectMatch(m))
  }
  const close = (): void => {
    if (view) {
      hideMatches(view)
      view.focus()
    }
    p.onClose()
  }
  const replaceOne = (): void => {
    if (!view || none || !canReplace) return
    const s = replaceOneSpec(view.state, spec)
    if (s) view.dispatch(s)
  }
  const replaceAll = (): void => {
    if (!view || none || !canReplace) return
    const s = replaceAllSpec(view.state, spec)
    if (s) view.dispatch(s)
  }
  const keys = (e: React.KeyboardEvent, enter: (shift: boolean) => void): void => {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      close()
    } else if (e.key === 'Enter') {
      e.preventDefault()
      enter(e.shiftKey)
    }
  }
  const nav = (label: string, dir: 1 | -1, glyph: string): React.JSX.Element => (
    <button
      type="button"
      data-k2-focus=""
      data-k2-chamfer="control"
      aria-label={label}
      aria-disabled={none || undefined}
      aria-describedby={none ? 'ed-find-count' : undefined}
      title={none ? offWhy : `${label} (${dir === 1 ? '⌘G' : '⇧⌘G'})`}
      onClick={() => !none && step(dir)}
      className="ed-find-btn lv-icon-btn no-drag"
    >
      <ToolGlyph name={glyph} />
    </button>
  )

  return (
    <div role="search" aria-label={`Find in ${p.fileName}`} data-testid="ed-find" className="ed-find">
      <div className="ed-frow">
        <span aria-hidden="true" className="ed-fic">
          <ToolGlyph name="search" />
        </span>
        <input
          ref={field}
          type="text"
          aria-label="Find"
          placeholder="Find"
          spellCheck={false}
          autoComplete="off"
          data-k2-chamfer="control"
          className="ed-fld no-drag"
          value={spec.search}
          onChange={(e) => setSpec({ ...spec, search: e.target.value })}
          onKeyDown={(e) => keys(e, (shift) => step(shift ? -1 : 1))}
        />
        <span id="ed-find-count" data-testid="ed-find-count" aria-live="polite" className={cx('ed-fcount', none && !count.empty ? 'ed-fcount-none' : '')}>
          {words}
        </span>
        {nav('Previous match', -1, 'up')}
        {nav('Next match', 1, 'down')}
        <Toggle label="Match case" on={spec.caseSensitive} onClick={() => setSpec({ ...spec, caseSensitive: !spec.caseSensitive })}>
          Aa
        </Toggle>
        <Toggle label="Whole words" on={spec.wholeWord} onClick={() => setSpec({ ...spec, wholeWord: !spec.wholeWord })}>
          W
        </Toggle>
        <Toggle label="Regular expression" on={spec.regexp} onClick={() => setSpec({ ...spec, regexp: !spec.regexp })}>
          .*
        </Toggle>
        <button
          type="button"
          data-k2-focus=""
          data-k2-chamfer="control"
          aria-label={p.replaceOpen ? 'Hide replace' : 'Show replace'}
          aria-pressed={p.replaceOpen}
          aria-disabled={!canReplace || undefined}
          title={canReplace ? 'Replace (⌥⌘F)' : roWhy}
          onClick={() => canReplace && p.setReplaceOpen(!p.replaceOpen)}
          className="ed-find-btn lv-icon-btn no-drag"
        >
          <ToolGlyph name="replace" />
        </button>
        <span className="flex-1" />
        <button type="button" data-k2-focus="" data-k2-chamfer="control" aria-label="Close find" title="Close (Esc)" onClick={close} className="ed-find-btn lv-icon-btn no-drag">
          <ToolGlyph name="close" />
        </button>
      </div>
      {p.replaceOpen && canReplace ? (
        <div className="ed-frow">
          <span aria-hidden="true" className="ed-fic">
            <ToolGlyph name="replace" />
          </span>
          <input
            ref={replaceField}
            type="text"
            aria-label="Replace with"
            placeholder="Replace with"
            spellCheck={false}
            autoComplete="off"
            data-k2-chamfer="control"
            className="ed-fld no-drag"
            value={spec.replace}
            onChange={(e) => setSpec({ ...spec, replace: e.target.value })}
            onKeyDown={(e) => keys(e, () => replaceOne())}
          />
          {(['Replace', 'Replace all'] as const).map((label) => (
            <button
              key={label}
              type="button"
              data-k2-focus=""
              data-k2-chamfer="control"
              aria-disabled={none || undefined}
              title={none ? offWhy : label === 'Replace' ? 'Replace this match and go to the next' : 'Replace every match, as one edit you can undo'}
              onClick={label === 'Replace' ? replaceOne : replaceAll}
              className="ed-find-act no-drag"
            >
              {label}
            </button>
          ))}
          <span className="flex-1" />
        </div>
      ) : null}
    </div>
  )
}
