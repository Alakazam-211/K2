// A plain file in the new editors, saving (build-editors.md E1c): EditorTab
// (markdown or code) with the save model, handed to the shared frame through
// FrameSaveContext. Autosave, ⌘S and blur write with the disk check; the
// failure, restore and merged notices sit under the header; Compare takes
// the text's place; the kept pair is marked in whichever editor is mounted.
// FileViewerPane keeps the file's text (base) and the buffer; this keeps
// what saving them means.

import { lazy, Suspense, useEffect, useMemo, useRef } from 'react'
import { EditorView } from '@codemirror/view'
import type { ServerScope } from '@/kessel/server-scope'
import { applyKeptMarks } from '@/lib/editor/save-model/kept-marks'
import type { KeptPair } from '@/lib/editor/save-model/merge3'
import { useSaveModel } from '@/lib/editor/save-model/useSaveModel'
import { EditorTab, type EditorTabProps } from './EditorTab'
import { FrameSaveContext, type FrameSave } from './frameSave'
import { SaveNoticeBar } from './SaveNoticeBar'
import { SAVE_CSS } from './saveCss'

// Compare brings @codemirror/merge: loaded when first opened.
const CompareView = lazy(() => import('./CompareView').then((m) => ({ default: m.CompareView })))

export interface EditorFileProps extends Omit<EditorTabProps, 'dirty' | 'saving' | 'onSave'> {
  scope: ServerScope
  /** The file's text as K2 last read it. */
  base: string
  /** K2 saw no file there: the first write expects none. */
  baseAbsent: boolean
  /** Not text (or K2 couldn't read it): never write. */
  locked: boolean
  setText: (text: string | null) => void
  setBase: (text: string) => void
  /** An Untitled document's first save (it picks a free name). */
  saveNew?: (text: string) => Promise<void>
}

/** Put the kept marks on the mounted editor whenever it or the merge changes. */
function useKeptMarks(root: React.RefObject<HTMLDivElement | null>, kept: KeptPair[] | null): void {
  const applied = useRef<{ view: EditorView | null; kept: KeptPair[] | null }>({ view: null, kept: null })
  // A passive effect: it runs after the editor's own (children first), so the merged text is in.
  useEffect(() => {
    // The text's editor, never Compare's.
    const el = [...(root.current?.querySelectorAll<HTMLElement>('.ed-body .cm-editor') ?? [])].find((e) => !e.closest('.ed-compare'))
    const view = el ? EditorView.findFromDOM(el) : null
    if (!view || (applied.current.view === view && applied.current.kept === kept)) return
    applied.current = { view, kept }
    applyKeptMarks(view, kept ?? [])
  })
  // The tags sit in the right margin only where they fit there; else on a row above (saveCss).
  useEffect(() => {
    const tab = root.current?.querySelector<HTMLElement>('[data-testid="editor-tab"]')
    if (!tab || !kept?.length) return
    const fit = (): void => {
      const content = tab.querySelector<HTMLElement>('.ed-body .cm-content')
      const scroller = tab.querySelector<HTMLElement>('.ed-body .cm-scroller')
      const room = content && scroller ? scroller.getBoundingClientRect().right - content.getBoundingClientRect().right : 0
      tab.toggleAttribute('data-kept-row', room < KEPT_TAG_ROOM_PX)
    }
    fit()
    const ro = typeof ResizeObserver === 'function' ? new ResizeObserver(fit) : null
    ro?.observe(tab)
    return () => {
      ro?.disconnect()
      tab.removeAttribute('data-kept-row')
    }
  }, [root, kept])
}

/** The right margin the widest tag ("Kept both · saved outside K2") needs, with its gap. */
const KEPT_TAG_ROOM_PX = 250

export function EditorFile({ scope, base, baseAbsent, locked, setText, setBase, saveNew, notice, ...tab }: EditorFileProps): React.JSX.Element {
  const ro = tab.readOnly !== null || locked
  const model = useSaveModel({ scope, filePath: tab.filePath, base, baseAbsent, text: tab.text, locked: ro, saveNew, setText, setBase })
  const root = useRef<HTMLDivElement>(null)
  useKeptMarks(root, model.merge?.kept ?? null)
  const { status, saveNow, onBlur, comparing, merge } = model
  const frameSave = useMemo<FrameSave>(
    () => ({
      status,
      saveNow,
      onLeave: onBlur,
      replaceBody:
        comparing && merge ? (
          <Suspense fallback={null}>
            <CompareView before={merge.before} now={tab.text} width={tab.width} />
          </Suspense>
        ) : null,
    }),
    [status, saveNow, onBlur, comparing, merge, tab.text, tab.width],
  )
  return (
    <div ref={root} className="contents" data-testid="editor-file" data-save-phase={model.state.phase} data-dirty={model.state.dirty ? '' : undefined}>
      <style>{SAVE_CSS}</style>
      <FrameSaveContext.Provider value={frameSave}>
        <EditorTab
          {...tab}
          dirty={model.state.dirty}
          saving={model.state.phase === 'saving'}
          onSave={saveNow}
          notice={
            <>
              {notice}
              {model.notice ? <SaveNoticeBar view={model.notice} kind={model.noticeKind ?? ''} onAction={model.onAction} /> : null}
            </>
          }
        />
      </FrameSaveContext.Provider>
    </div>
  )
}
