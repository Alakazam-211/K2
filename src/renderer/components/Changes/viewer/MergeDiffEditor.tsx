// One read-only CodeMirror diff: inline (`unifiedMergeView`, one editor on
// the new text with the old lines shown above) or split (`MergeView`, old
// left / new right, unchanged lines aligned).
//
// Language, syntax colours, font and the editor theme are the user's
// code-editor settings (shared with CodeEditor); diff marks use K2 tokens
// (diffTheme.ts). Unchanged stretches collapse; word wrap toggles live.

import { useEffect, useMemo, useRef } from 'react'
import { EditorView, lineNumbers, drawSelection, highlightSpecialChars, keymap } from '@codemirror/view'
import { EditorState, Compartment, type Extension } from '@codemirror/state'
import { search, searchKeymap, highlightSelectionMatches } from '@codemirror/search'
import { MergeView, unifiedMergeView, getChunks, type Chunk } from '@codemirror/merge'
import { useSettingsStore } from '@/stores/settings'
import { useCustomThemesStore } from '@/stores/custom-themes'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import {
  getLanguageExtension,
  getThemeExtension,
  getHighlightExtension,
  buildFontFamilyExtension,
} from '@/components/FileViewerPane/CodeEditor'
import { k2DiffTheme, autoHeightTheme, fillHeightTheme } from './diffTheme'
import { notesExtension, setNoteMarks, type NoteMark, type NotesHandlers } from './notesExtension'
import type { ChangesDiffLayout } from './types'

/** Review notes for this editor (C5): placed marks per side and the
 *  callbacks. The old side only shows in split view. */
export interface EditorNotes {
  marks: { old: NoteMark[]; new: NoteMark[] }
  canWrite: boolean
  handlers: (side: 'old' | 'new') => NotesHandlers
}

/** Imperative handle for next/previous change. Positions are in the NEW
 *  text (side b), which is what both layouts navigate. */
export interface MergeDiffApi {
  goTo(pos: number): void
  cursor(): number
  focus(): void
}

export interface MergeDiffEditorProps {
  oldText: string
  newText: string
  /** For language detection (the new path on a rename). */
  filePath: string
  layout: ChangesDiffLayout
  wrap: boolean
  collapse: boolean
  /** Single view: fill the pane and scroll inside. View all: grow to the
   *  content; the stacked list scrolls. */
  fill: boolean
  /** Chunk start positions (side b) after every (re)build. */
  onChunks?: (starts: number[]) => void
  /** The cursor moved or the editor took focus (position on side b). */
  onCursor?: (pos: number) => void
  apiRef?: React.MutableRefObject<MergeDiffApi | null>
  /** Review notes (C5); absent = no notes gutter. */
  notes?: EditorNotes
}

/** The diff algorithm gives up on precise diffs after this long and falls
 *  back to a fast line diff (keeps a huge rewrite from freezing the UI). */
const DIFF_CONFIG = { scanLimit: 500, timeout: 1500 }
const COLLAPSE = { margin: 3, minSize: 6 }

export function chunkStarts(chunks: readonly Chunk[]): number[] {
  return chunks.map((c) => c.fromB)
}

function useEditorLook(): { theme: Extension; highlight: Extension; font: Extension } {
  const themeId = useSettingsStore((s) => s.editor.theme) || 'k2so-dark'
  const size = useSettingsStore((s) => s.editor.fontSize) || 13
  const family = useSettingsStore((s) => s.editor.fontFamily) || 'MesloLGM Nerd Font'
  const ligatures = useSettingsStore((s) => s.editor.fontLigatures) ?? false
  // Rebuild when custom themes load so a `custom:` theme id resolves.
  const customThemes = useCustomThemesStore((s) => s.customThemes)
  const theme = useMemo(() => getThemeExtension(themeId), [themeId, customThemes])
  const highlight = useMemo(() => getHighlightExtension(themeId), [themeId, customThemes])
  const font = useMemo(
    () => [
      buildFontFamilyExtension(family, ligatures),
      EditorView.theme({ '&': { fontSize: `${size}px` }, '.cm-gutters': { fontSize: `${Math.max(size - 1, 10)}px` } }),
    ],
    [family, ligatures, size],
  )
  return { theme, highlight, font }
}

export function MergeDiffEditor(props: MergeDiffEditorProps): React.JSX.Element {
  const { oldText, newText, filePath, layout, wrap, collapse, fill, onChunks, onCursor, apiRef, notes } = props
  const hostRef = useRef<HTMLDivElement>(null)
  const viewsRef = useRef<{ nav: EditorView; all: EditorView[]; destroy: () => void; old: EditorView | null } | null>(null)
  // Notes callbacks are read at event time; the editor never rebuilds for them.
  const noteHandlers = useRef({ old: { current: null as NotesHandlers | null }, new: { current: null as NotesHandlers | null } })
  noteHandlers.current.old.current = notes ? notes.handlers('old') : null
  noteHandlers.current.new.current = notes ? notes.handlers('new') : null
  const notesNow = useRef(notes)
  notesNow.current = notes
  const withNotes = !!notes
  const lookRef = useRef<{ theme: Compartment; wrap: Compartment }>({ theme: new Compartment(), wrap: new Compartment() })
  const onChunksRef = useRef(onChunks)
  onChunksRef.current = onChunks
  const onCursorRef = useRef(onCursor)
  onCursorRef.current = onCursor
  const look = useEditorLook()
  const lookNow = useRef(look)
  lookNow.current = look
  const wrapNow = useRef(wrap)
  wrapNow.current = wrap
  const visible = useIsTabVisible()

  // Build (and rebuild) the editor(s). Text, layout, collapse and language
  // need a fresh state; theme and wrap reconfigure in place below.
  useEffect(() => {
    const host = hostRef.current
    if (!host) return
    const { theme: themeC, wrap: wrapC } = lookRef.current
    const l = lookNow.current
    const lang = getLanguageExtension(filePath)
    const shared = (): Extension[] => [
      EditorState.readOnly.of(true),
      lineNumbers(),
      highlightSpecialChars(),
      drawSelection(),
      highlightSelectionMatches(),
      search({ top: true }),
      keymap.of(searchKeymap),
      lang ?? [],
      themeC.of([l.theme, l.highlight, l.font]),
      wrapC.of(wrapNow.current ? EditorView.lineWrapping : []),
      k2DiffTheme,
      fill && layout === 'inline' ? fillHeightTheme : autoHeightTheme,
    ]
    const cursorListener = EditorView.updateListener.of((u) => {
      if (u.selectionSet || (u.focusChanged && u.view.hasFocus)) {
        onCursorRef.current?.(u.state.selection.main.head)
      }
    })

    let nav: EditorView
    let all: EditorView[]
    let destroy: () => void
    let starts: () => number[]
    let oldView: EditorView | null = null
    if (layout === 'split') {
      const mv = new MergeView({
        a: { doc: oldText, extensions: [shared(), withNotes ? notesExtension(noteHandlers.current.old) : []] },
        b: { doc: newText, extensions: [shared(), cursorListener, withNotes ? notesExtension(noteHandlers.current.new) : []] },
        parent: host,
        highlightChanges: true,
        gutter: true,
        collapseUnchanged: collapse ? COLLAPSE : undefined,
        diffConfig: DIFF_CONFIG,
      })
      if (fill) {
        mv.dom.style.height = '100%'
        mv.dom.style.overflowY = 'auto'
      }
      nav = mv.b
      all = [mv.a, mv.b]
      oldView = mv.a
      destroy = () => mv.destroy()
      starts = () => chunkStarts(mv.chunks)
    } else {
      const view = new EditorView({
        parent: host,
        state: EditorState.create({
          doc: newText,
          extensions: [
            shared(),
            cursorListener,
            withNotes ? notesExtension(noteHandlers.current.new) : [],
            unifiedMergeView({
              original: oldText,
              highlightChanges: true,
              gutter: true,
              mergeControls: false,
              syntaxHighlightDeletions: true,
              collapseUnchanged: collapse ? COLLAPSE : undefined,
              diffConfig: DIFF_CONFIG,
            }),
          ],
        }),
      })
      nav = view
      all = [view]
      destroy = () => view.destroy()
      starts = () => chunkStarts(getChunks(view.state)?.chunks ?? [])
    }
    viewsRef.current = { nav, all, destroy, old: oldView }
    const n = notesNow.current
    if (n) {
      nav.dispatch({ effects: setNoteMarks.of({ marks: n.marks.new, canWrite: n.canWrite }) })
      oldView?.dispatch({ effects: setNoteMarks.of({ marks: n.marks.old, canWrite: n.canWrite }) })
    }
    if (apiRef) {
      apiRef.current = {
        goTo(pos) {
          const p = Math.min(Math.max(pos, 0), nav.state.doc.length)
          nav.dispatch({
            selection: { anchor: p },
            effects: EditorView.scrollIntoView(p, { y: 'center' }),
            userEvent: 'select.byChunk',
          })
          nav.focus()
        },
        cursor: () => nav.state.selection.main.head,
        focus: () => nav.focus(),
      }
    }
    onChunksRef.current?.(starts())
    return () => {
      if (apiRef) apiRef.current = null
      viewsRef.current = null
      destroy()
    }
    // `apiRef` is a stable ref object from the parent.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [oldText, newText, filePath, layout, collapse, fill, withNotes])

  // Notes moved or changed: hand the editors their marks.
  const marksNew = notes?.marks.new
  const marksOld = notes?.marks.old
  const canWriteNotes = notes?.canWrite ?? false
  useEffect(() => {
    const v = viewsRef.current
    if (!v || !marksNew) return
    v.nav.dispatch({ effects: setNoteMarks.of({ marks: marksNew, canWrite: canWriteNotes }) })
    v.old?.dispatch({ effects: setNoteMarks.of({ marks: marksOld ?? [], canWrite: canWriteNotes }) })
  }, [marksNew, marksOld, canWriteNotes])

  // Theme / font / wrap: reconfigure in place.
  useEffect(() => {
    const v = viewsRef.current
    if (!v) return
    const { theme: themeC, wrap: wrapC } = lookRef.current
    for (const view of v.all) {
      view.dispatch({
        effects: [
          themeC.reconfigure([look.theme, look.highlight, look.font]),
          wrapC.reconfigure(wrap ? EditorView.lineWrapping : []),
        ],
      })
    }
  }, [look.theme, look.highlight, look.font, wrap])

  // Mounted inside a hidden tab: measure again once shown.
  useEffect(() => {
    if (!visible) return
    for (const view of viewsRef.current?.all ?? []) view.requestMeasure()
  }, [visible])

  return <div ref={hostRef} className={fill ? 'h-full min-h-0' : ''} data-testid="merge-diff-editor" data-layout={layout} />
}
