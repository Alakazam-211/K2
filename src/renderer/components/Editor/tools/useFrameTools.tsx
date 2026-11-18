// The frame's E3 tools in one place (build-editors.md E3), so EditorFrame
// only places them: the find bar (⌘F, ⌥⌘F with Replace, ⌘G and ⇧⌘G), go to
// line (⌃G and the status line's Ln, Col), and in Write the formatting
// toolbar over a selection (⌥F10 takes the keyboard to it) and the / menu.
// They all act on the view the frame's store holds (lib/editor/frame-view).
//
// The toolbar sits in the right margin beside the selection's first line, 20
// px from the column, so it never covers text (editors-notes.md v2); with no
// margin for it (narrow, or the panel open at 800) it docks in the file bar
// in place of the sentence (w17). The / menu opens under the "/" line.

import { useCallback, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import type { EditorView } from '@codemirror/view'
import { clientToCssPx } from '@/lib/app-zoom'
import { EMPTY_FIND, type FindSpec } from '@/lib/editor/find/find'
import { createFrameView, frameExtension, type FrameView } from '@/lib/editor/frame-view'
import { formatSpec, formatState, type FormatId } from '@/lib/editor/md-commands'
import { slashState } from '@/lib/editor/slash/slash'
import { FindBar } from './FindBar'
import { FormatToolbar } from './FormatToolbar'
import { GoToLine } from './GoToLine'
import { SlashMenu } from './SlashMenu'

/** The toolbar's size before it's measured (six 30px buttons, the rule, the padding). */
const FMT_W = 212
const FMT_H = 40
const FMT_GAP = 20

/** The frame's store and each mode's extension for its view (made once per frame). */
export function useFrameStore(): { store: FrameView; extensionFor: (write: boolean) => ReturnType<typeof frameExtension> } {
  const [store] = useState(createFrameView)
  const write = useMemo(() => frameExtension(store, { write: true }), [store])
  const other = useMemo(() => frameExtension(store, { write: false }), [store])
  return { store, extensionFor: (w) => (w ? write : other) }
}

export function useFrameView(store: FrameView): { view: EditorView | null; version: number } {
  const version = useSyncExternalStore(store.subscribe, store.version, store.version)
  return { view: store.get(), version }
}

type Place = { left: number; top: number } | 'dock' | null
type SlashPlace = { left: number; top: number; maxHeight: number | null }

/** The / menu's height before it's measured (the help line and eight rows). */
const SLASH_H = 340

export interface FrameToolsOptions {
  store: FrameView
  fileName: string
  mode: string
  /** Why the file can't be edited, or null. */
  readOnly: string | null
}

export function useFrameTools(o: FrameToolsOptions) {
  const { view, version } = useFrameView(o.store)
  const [find, setFind] = useState<{ open: boolean; key: number; replace: boolean; focusReplace: boolean }>({ open: false, key: 0, replace: false, focusReplace: false })
  const [spec, setSpec] = useState<FindSpec>(EMPTY_FIND)
  const [goto, setGoto] = useState(false)
  const hero = useRef<HTMLDivElement>(null)
  const fmtRef = useRef<HTMLDivElement>(null)
  const [fmtPlace, setFmtPlace] = useState<Place>(null)
  const [slashPlace, setSlashPlace] = useState<SlashPlace | null>(null)
  const slashRef = useRef<HTMLDivElement>(null)

  const write = o.mode === 'write' && o.readOnly === null && !!view && !view.state.readOnly
  const sel = view?.state.selection.main
  const slash = write && view ? slashState(view.state) : null
  const fmtWanted = write && !!sel && !sel.empty && !find.open && !slash

  const openFind = useCallback(
    (replace: boolean) => {
      if (!view) return
      const s = view.state.selection.main
      const words = !s.empty && s.to - s.from <= 100 ? view.state.sliceDoc(s.from, s.to) : ''
      if (words && !words.includes('\n')) setSpec((p) => ({ ...p, search: words }))
      setFind((f) => ({ open: true, key: f.key + 1, replace: replace || f.replace, focusReplace: replace }))
    },
    [view],
  )
  const closeFind = useCallback(() => setFind((f) => ({ ...f, open: false })), [])

  /** The tab's keys (capture phase). True when one was taken. */
  const onKey = (e: React.KeyboardEvent): boolean => {
    if (!view) return false
    const mod = e.metaKey || e.ctrlKey
    const k = e.key.toLowerCase()
    const take = (): true => {
      e.preventDefault()
      e.stopPropagation()
      return true
    }
    if (mod && k === 'f' && !e.shiftKey) {
      openFind(e.altKey)
      return take()
    }
    // ⌃G: go to line (⌘L is the app's server switcher). ⌘G, ⇧⌘G and F3: the next and previous match.
    if (e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey && k === 'g') {
      setGoto(true)
      return take()
    }
    if ((e.metaKey && k === 'g' && !e.altKey) || e.key === 'F3') {
      if (!find.open || !spec.search) openFind(false)
      else hero.current?.closest('[data-testid="editor-tab"]')?.querySelector<HTMLButtonElement>(`[data-testid="ed-find"] [aria-label="${e.shiftKey ? 'Previous match' : 'Next match'}"]`)?.click()
      return take()
    }
    if (e.altKey && e.key === 'F10' && fmtRef.current) {
      fmtRef.current.querySelector<HTMLButtonElement>('button[tabindex="0"]')?.focus()
      return take()
    }
    return false
  }

  // Where the toolbar and the / menu go, after every update of the view and as it scrolls.
  useLayoutEffect(() => {
    const host = hero.current
    if (!view || !host || (!fmtWanted && !slash)) {
      setFmtPlace(null)
      setSlashPlace(null)
      return
    }
    const place = (): void => {
      const hr = host.getBoundingClientRect()
      const sr = view.scrollDOM.getBoundingClientRect()
      if (fmtWanted && sel) {
        const cr = view.contentDOM.getBoundingClientRect()
        const w = fmtRef.current && !fmtRef.current.dataset.docked ? fmtRef.current.offsetWidth || FMT_W : FMT_W
        const h = fmtRef.current && !fmtRef.current.dataset.docked ? fmtRef.current.offsetHeight || FMT_H : FMT_H
        const room = clientToCssPx(sr.right - cr.right) - FMT_GAP - w - 8
        if (room < 0) setFmtPlace('dock')
        else {
          const first = Math.max(sel.from, view.viewport.from)
          const at = first <= view.viewport.to ? view.coordsAtPos(first, 1) : null
          const mid = at ? (at.top + at.bottom) / 2 : null
          if (mid === null || mid < sr.top || mid > sr.bottom) setFmtPlace(null)
          else {
            const top = clientToCssPx(mid - hr.top) - h / 2
            const lo = clientToCssPx(sr.top - hr.top) + 4
            const hi = clientToCssPx(sr.bottom - hr.top) - h - 4
            setFmtPlace({ left: Math.round(clientToCssPx(cr.right - hr.left) + FMT_GAP), top: Math.round(Math.min(Math.max(top, lo), Math.max(lo, hi))) })
          }
        }
      } else setFmtPlace(null)
      if (slash) {
        const at = view.coordsAtPos(slash.from, 1)
        const width = 280
        const left = at ? Math.min(Math.max(8, clientToCssPx(at.left - hr.left) - 10), Math.max(8, clientToCssPx(hr.width) - width - 8)) : 8
        // It stays inside the text's scroll area: under the line when it fits, else over it,
        // else on the roomier side with its list capped (it scrolls).
        const lo = clientToCssPx(sr.top - hr.top) + 4
        const hi = clientToCssPx(sr.bottom - hr.top) - 4
        const below = at ? clientToCssPx(at.bottom - hr.top) + 6 : lo
        const aboveEnd = at ? clientToCssPx(at.top - hr.top) - 6 : lo
        const h = slashRef.current?.scrollHeight || SLASH_H
        const roomBelow = hi - below
        const roomAbove = aboveEnd - lo
        const place: SlashPlace =
          h <= roomBelow ? { left, top: below, maxHeight: null }
          : h <= roomAbove ? { left, top: aboveEnd - h, maxHeight: null }
          : roomBelow >= roomAbove ? { left, top: below, maxHeight: Math.max(80, roomBelow) }
          : { left, top: Math.max(lo, aboveEnd - roomAbove), maxHeight: Math.max(80, roomAbove) }
        setSlashPlace({ ...place, left: Math.round(place.left), top: Math.round(place.top) })
      } else setSlashPlace(null)
    }
    place()
    let frame = 0
    const onScroll = (): void => {
      if (!frame) frame = requestAnimationFrame(() => ((frame = 0), place()))
    }
    view.scrollDOM.addEventListener('scroll', onScroll, { passive: true })
    return () => {
      view.scrollDOM.removeEventListener('scroll', onScroll)
      if (frame) cancelAnimationFrame(frame)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, version, fmtWanted, !!slash, slash?.query])

  const run = (id: FormatId): void => {
    if (!view) return
    const s = formatSpec(view.state, id)
    if (s) view.dispatch(s)
  }
  const toolbar = (docked: boolean): React.JSX.Element | null =>
    view && fmtWanted ? <FormatToolbar ref={fmtRef} docked={docked} active={formatState(view.state)} onRun={run} onEscape={() => view.focus()} style={docked || fmtPlace === 'dock' || !fmtPlace ? undefined : { position: 'absolute', left: fmtPlace.left, top: fmtPlace.top }} /> : null

  return {
    version,
    onKey,
    findBar: find.open && view ? <FindBar store={o.store} version={version} fileName={o.fileName} spec={spec} setSpec={setSpec} replaceOpen={find.replace} setReplaceOpen={(r) => setFind((f) => ({ ...f, replace: r, focusReplace: r }))} readOnly={o.readOnly} focusKey={find.key} focusReplace={find.focusReplace} onClose={closeFind} /> : null,
    /** The hero's own box: the floats are placed against it. */
    hero,
    floats: (
      <>
        {fmtPlace && fmtPlace !== 'dock' ? toolbar(false) : null}
        {view && slash ? <SlashMenu ref={slashRef} store={o.store} state={slash} id={o.store.slashId} style={{ position: 'absolute', left: slashPlace?.left ?? 8, top: slashPlace?.top ?? 8, ...(slashPlace?.maxHeight ? { maxHeight: slashPlace.maxHeight } : {}), visibility: slashPlace ? undefined : 'hidden' }} /> : null}
      </>
    ),
    /** In the file bar when the margin has no room. */
    docked: fmtPlace === 'dock' ? toolbar(true) : null,
    goto: goto && view ? <GoToLine store={o.store} onClose={() => setGoto(false)} /> : null,
    openGoto: view ? () => setGoto(true) : null,
  }
}
