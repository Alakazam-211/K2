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
// FrameLiveContext (frameLive.ts). E3's tools act on the view the editor
// mounts (its `frame` extension feeds the frame's store): the docked find bar,
// go to line from Ln, Col, the formatting toolbar and the / menu in Write
// (tools/useFrameTools), and the Outline in the side panel for markdown
// (outline/useFramePanel).

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
import { useFramePanel } from './outline/useFramePanel'
import { useFrameStore, useFrameTools } from './tools/useFrameTools'
import { TOOLS_CSS } from './tools/toolsCss'

/** Narrower than this, the status line's toggles fold behind Display. */
export const EDITOR_NARROW_BELOW = 900

export interface EditorMountOptions {
  mode: EditorMode
  readOnly: boolean
  lineNumbers: boolean
  wrap: boolean
  /** Write and Read: the rendered page (lib/editor/write-mode). Source: none. */
  extensions: Extension
  /** The frame's tools on this view (find, the outline, Write's toolbar and / menu): mount it just before `extensions`,
   *  so a find match's mark sits inside Write's marks. */
  frame?: Extension
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
  const tools = useFrameStore()
  const refocus = useRef(false)
  useEffect(() => {
    if (!refocus.current) return
    refocus.current = false
    root.current?.querySelector<HTMLElement>('.cm-content')?.focus()
  }, [mode, root])
  return { root, narrow: tabWidth < EDITOR_NARROW_BELOW, refocusAfterModeChange: () => void (refocus.current = true), store: tools.store, extensionFor: tools.extensionFor }
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
  /** Why the file can't be edited, or null: find can't replace. */
  readOnly?: string | null
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
  const name = fileNameOf(filePath)
  const tools = useFrameTools({ store: frame.store, fileName: name, mode, readOnly: props.readOnly ?? null })
  const side = useFramePanel({ kind, live, view: tools.view, version: tools.version, narrow: frame.narrow })
  // Ln, Col opens go to line.
  const pos = (it: StatusItem): StatusItem => (it.id === 'pos' && tools.openGoto ? { id: 'pos', node: <button type="button" data-k2-focus="" data-k2-chamfer="chip" title="Go to line (⌘L)" onClick={tools.openGoto} className="ed-sb no-drag">{it.node}</button> } : it)
  const items = (save ? status.items.flatMap((it) => (it.id !== 'save' ? [it] : word ? [word] : [])) : status.items).map(pos)
  // A file Write can't edit may still owe its words (a live file whose edits are paused): say so.
  if (word && !status.items.some((it) => it.id === 'save')) items.push(word)
  const onKeyDownCapture = (e: React.KeyboardEvent): void => {
    if (save && saveKey(e, save.saveNow)) return
    if (tools.onKey(e)) return
    props.onKeyDownCapture?.(e)
  }
  const onBlur = (e: React.FocusEvent): void => {
    if (save && !frame.root.current?.contains(e.relatedTarget as Node | null)) save.onLeave()
  }
  return (
    // data-k2-line: the mode control measures its room against the tab, not its own wrapper (ui/segmented-fit.ts).
    <div ref={frame.root} data-testid="editor-tab" data-k2-line="" data-kind={kind} data-mode={mode} data-full={full ? '' : undefined} data-live={live ? '' : undefined} data-offline={live?.offline ? '' : undefined} style={props.style} className="ed-tab flex h-full min-h-0 flex-col" onKeyDownCapture={onKeyDownCapture} onBlur={onBlur}>
      <style>{EDITOR_CSS + WRITE_CSS + TOOLS_CSS}</style>
      <LiveTabFrame
        header={<LiveHeader fileName={name} fileTitle={filePath} fileGlyph={<SetiFileIcon name={name} size={16} />} state={live?.state} sentence={live?.sentence ?? null} sentenceEnd={live?.sentenceEnd} fitKey={live?.fitKey} right={live?.right} mode={props.modeControl} docked={tools.docked} panelOpen={side.panelOpen} onTogglePanel={side.onTogglePanel} panelLabel={side.panelLabel} />}
        notice={props.notice}
        findBar={save?.replaceBody ? null : tools.findBar}
        hero={
          <div ref={tools.hero} className="ed-body relative flex min-h-0 flex-1 flex-col">
            {save?.replaceBody ?? props.body}
            {save?.replaceBody ? null : tools.floats}
          </div>
        }
        status={
          <div className="relative flex-none">
            {tools.goto}
            <EditorStatusLine items={items} toggles={status.toggles} facts={status.facts} narrow={frame.narrow} />
          </div>
        }
        panel={side.panel}
        panelOpen={side.panelOpen}
        panelMode="reflow"
        panelLabel={side.panelLabel}
        narrow={frame.narrow}
        sample={false}
        onClosePanel={side.onClosePanel}
      />
    </div>
  )
}
