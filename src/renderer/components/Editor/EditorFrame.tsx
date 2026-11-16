// The frame both new file editors share (build-editors.md E1b): MarkdownEditor
// and CodeEditorTab put their body and their status line in it. One slim
// header: the file's tile, its name once (the path is the name's tooltip) and
// the editor's mode control, if it has one. No toolbar and no path bar: the
// file reloads on its own and the tab closes itself. Under the text, the
// status line (EditorStatusLine), which says the save state as it is. Saving
// (E1c) comes from EditorFile through FrameSaveContext: the save word, ⌘S in
// every mode, autosave on blur and Compare, the same for both editors.
// The LiveView shell's slots hold it all. A live file (E4a) adds the state
// chip, the sentence, the avatar stack and the Live panel through
// FrameLiveContext (frameLive.ts).

import { useContext, useEffect, useRef } from 'react'
import type { Extension } from '@codemirror/state'
import { LiveHeader } from '@/components/LiveView/shell/LiveHeader'
import { LiveTabFrame } from '@/components/LiveView/shell/LiveTabFrame'
import { useTabWidth } from '@/components/LiveView/shell/useLiveShell'
import { SetiFileIcon } from '@/lib/seti-file-icons/SetiFileIcon'
import type { EditorMode } from '@/lib/editor/modes'
import type { EditorKind } from '@/lib/editor/preview-switch'
import { EDITOR_CSS } from './editorCss'
import { WRITE_CSS } from './writeModeCss'
import { EditorStatusLine, SaveWord, type StatusItem, type StatusToggle } from './EditorStatusLine'
import { FrameLiveContext } from './frameLive'
import { FrameSaveContext } from './frameSave'

/** Narrower than this, the status line's toggles fold behind Display. */
export const EDITOR_NARROW_BELOW = 900

export interface EditorMountOptions {
  mode: EditorMode
  readOnly: boolean
  lineNumbers: boolean
  wrap: boolean
  /** Write and Read: the rendered page (lib/editor/write-mode). Source: none. */
  extensions: Extension
}

/** What the file tab hands either editor. */
export interface EditorProps {
  filePath: string
  /** The buffer: unsaved edits included. */
  text: string
  dirty: boolean
  saving: boolean
  /** Why the file can't be edited, or null. */
  readOnly: string | null
  cursor: { line: number; col: number }
  /** Bars under the header (a re-read that failed, live editing turned off). */
  notice?: React.ReactNode
  /** The tab's width (tests); else measured. */
  width?: number
  /** ⌘S while no editor is mounted (an editor saves through itself). */
  onSave?: () => void
  /** The text editor (CodeMirror), for every mode that edits or reads text. */
  renderEditor: (o: EditorMountOptions) => React.ReactNode
  /** The file's own preview (CSV, HTML, Mermaid, diagram, DXF): the code editor's Preview. */
  renderPreview?: () => React.ReactNode
}

export const fileNameOf = (filePath: string): string => filePath.split('/').pop() || filePath

/** The frame's root, whether it is narrow, and a way to put the keyboard back in the text after a key moved the mode. */
export function useEditorFrame(width: number | undefined, mode: string) {
  const [root, tabWidth] = useTabWidth(width)
  const refocus = useRef(false)
  useEffect(() => {
    if (!refocus.current) return
    refocus.current = false
    root.current?.querySelector<HTMLElement>('.cm-content')?.focus()
  }, [mode, root])
  return { root, narrow: tabWidth < EDITOR_NARROW_BELOW, refocusAfterModeChange: () => void (refocus.current = true) }
}

/** The save word (none when the file can't be edited) and the caret's place: the same in both editors. */
export function saveItems(p: Pick<EditorProps, 'readOnly' | 'saving' | 'dirty'>): StatusItem[] {
  if (p.readOnly !== null) return []
  return [{ id: 'save', node: <SaveWord word={p.saving ? 'Saving' : p.dirty ? 'Not saved' : 'Saved'} /> }]
}
export const caretItem = (cursor: EditorProps['cursor']): StatusItem => ({ id: 'pos', node: `Ln ${cursor.line}, Col ${cursor.col}` })

/** ⌘S (Ctrl+S) while no editor holds the keyboard: the caller's save. True when it was. */
export function saveKey(e: React.KeyboardEvent, onSave: (() => void) | undefined): boolean {
  if (!((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === 's')) return false
  e.preventDefault()
  e.stopPropagation()
  onSave?.()
  return true
}

export function EditorFrame(props: {
  frame: ReturnType<typeof useEditorFrame>
  filePath: string
  kind: EditorKind
  mode: string
  full?: boolean
  /** The tab's own CSS variables (the markdown editor's `--ed-measure`). */
  style?: React.CSSProperties
  modeControl: React.ReactNode
  notice?: React.ReactNode
  body: React.ReactNode
  status: { items: StatusItem[]; toggles: StatusToggle[]; facts: StatusItem[] }
  onKeyDownCapture?: (e: React.KeyboardEvent) => void
}): React.JSX.Element {
  const { frame, filePath, kind, mode, full, status } = props
  const save = useContext(FrameSaveContext)
  const live = useContext(FrameLiveContext)
  // The save model's word replaces the editor's own (none for a read-only file).
  const word = save?.status ? { id: 'save', node: <SaveWord word={save.status.word} tone={save.status.tone} /> } : null
  const items = save ? status.items.flatMap((it) => (it.id !== 'save' ? [it] : word ? [word] : [])) : status.items
  // A file Write can't edit may still owe its words (a live file whose edits are paused): say so.
  if (word && !status.items.some((it) => it.id === 'save')) items.push(word)
  const onKeyDownCapture = (e: React.KeyboardEvent): void => {
    if (save && saveKey(e, save.saveNow)) return
    props.onKeyDownCapture?.(e)
  }
  const onBlur = (e: React.FocusEvent): void => {
    if (save && !frame.root.current?.contains(e.relatedTarget as Node | null)) save.onLeave()
  }
  const name = fileNameOf(filePath)
  return (
    // data-k2-line: the mode control measures its room against the tab, not its own wrapper (ui/segmented-fit.ts).
    <div ref={frame.root} data-testid="editor-tab" data-k2-line="" data-kind={kind} data-mode={mode} data-full={full ? '' : undefined} data-live={live ? '' : undefined} data-offline={live?.offline ? '' : undefined} style={props.style} className="ed-tab flex h-full min-h-0 flex-col" onKeyDownCapture={onKeyDownCapture} onBlur={onBlur}>
      <style>{EDITOR_CSS + WRITE_CSS}</style>
      <LiveTabFrame
        header={<LiveHeader fileName={name} fileTitle={filePath} fileGlyph={<SetiFileIcon name={name} size={16} />} state={live?.state} sentence={live?.sentence ?? null} sentenceEnd={live?.sentenceEnd} fitKey={live?.fitKey} right={live?.right} mode={props.modeControl} panelOpen={live?.panelOpen} onTogglePanel={live?.onTogglePanel} panelLabel={live?.panelLabel} />}
        notice={props.notice}
        hero={<div className="ed-body flex min-h-0 flex-1 flex-col">{save?.replaceBody ?? props.body}</div>}
        status={<EditorStatusLine items={items} toggles={status.toggles} facts={status.facts} narrow={frame.narrow} />}
        panel={live?.panel ?? null}
        panelOpen={!!live?.panelOpen}
        panelMode="reflow"
        panelLabel={live?.panelLabel}
        narrow={frame.narrow}
        sample={false}
        onClosePanel={live?.onClosePanel ?? (() => {})}
      />
    </div>
  )
}
