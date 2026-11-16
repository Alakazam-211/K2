// The file editor's frame on the live shell (build-editors.md E1b), behind
// "New file editor (preview)". One slim header: the file's tile, its name
// once (the path is the name's tooltip) and, for markdown, Write | Read |
// Source. No toolbar and no path bar: the file reloads on its own and the
// tab closes itself. Under the text, the status line (EditorStatusLine).
//
// The text is the caller's buffer. Every mode mounts the caller's editor
// (CodeMirror, through `renderEditor`): Write and Read with the write-mode
// extension (E2: the rendered page; Read is it, read only, with no caret),
// Source with none. A file over 1 MB or 20,000 lines is Source only. Saving
// stays the caller's (E1c brings autosave, the failure bar and the tab dot);
// the status line says the save state as it is. Nothing live yet (E4).

import { useEffect, useRef, useState } from 'react'
import type { Extension } from '@codemirror/state'
import { Segmented } from '@/components/ui'
import { LiveHeader } from '@/components/LiveView/shell/LiveHeader'
import { LiveTabFrame } from '@/components/LiveView/shell/LiveTabFrame'
import { useTabWidth } from '@/components/LiveView/shell/useLiveShell'
import { getLanguageName } from '@/components/FileViewerPane/CodeEditor'
import { SetiFileIcon } from '@/lib/seti-file-icons/SetiFileIcon'
import { useLocalFlag } from '@/lib/editor/local-flag'
import { initialMode, modeForKey, usableMode, type EditorMode } from '@/lib/editor/modes'
import type { EditorKind } from '@/lib/editor/preview-switch'
import { countWords, lineEnding, readingMinutes } from '@/lib/editor/words'
import { writeModeFits } from '@/lib/editor/write-mode'
import { useSettingsStore } from '@/stores/settings'
import { EDITOR_CSS } from './editorCss'
import { WRITE_CSS } from './writeModeCss'
import { useWriteMode } from './useWriteMode'
import { EditorStatusLine, SaveWord, type StatusItem, type StatusToggle } from './EditorStatusLine'

/** Narrower than this, the status line's toggles fold behind Display. */
export const EDITOR_NARROW_BELOW = 900
const FULL_WIDTH_KEY = 'k2.editor.fullWidth.v1'
const MARKDOWN_WRAP_KEY = 'k2.editor.markdownWrap.v1'
export const TOO_LARGE_REASON = 'This file is too large to show formatted (over 1 MB or 20,000 lines)'

export interface EditorMountOptions {
  mode: EditorMode
  readOnly: boolean
  lineNumbers: boolean
  wrap: boolean
  /** Write and Read: the rendered page (lib/editor/write-mode). Source: none. */
  extensions: Extension
}

export interface EditorTabProps {
  filePath: string
  kind: EditorKind
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
  /** ⌘S while reading (Write and Source save through the editor). */
  onSave?: () => void
  /** The text editor for Write and Source. */
  renderEditor: (o: EditorMountOptions) => React.ReactNode
}

const MODE_OPTIONS: { value: EditorMode; label: string }[] = [
  { value: 'write', label: 'Write' },
  { value: 'read', label: 'Read' },
  { value: 'source', label: 'Source' },
]

export function EditorTab({ filePath, kind, text, dirty, saving, readOnly, cursor, notice, width, onSave, renderEditor }: EditorTabProps): React.JSX.Element {
  const [root, tabWidth] = useTabWidth(width)
  const narrow = tabWidth < EDITOR_NARROW_BELOW
  const ro = readOnly !== null
  // Decided once per file: typing past the limit doesn't throw you out of Write.
  const [large] = useState(() => kind === 'markdown' && !writeModeFits(text))
  const [picked, setPicked] = useState<EditorMode>(() => initialMode(kind, ro, large))
  const mode = kind === 'code' ? 'source' : usableMode(picked, ro, large)
  const extensions = useWriteMode(kind === 'markdown' ? mode : 'source', filePath)
  const [full, setFull] = useLocalFlag(FULL_WIDTH_KEY)
  const [mdWrap, setMdWrap] = useLocalFlag(MARKDOWN_WRAP_KEY, true)
  const codeWrap = useSettingsStore((s) => s.editor.wordWrap)
  const lineNumbers = useSettingsStore((s) => s.editor.lineNumbers)
  const updateEditor = useSettingsStore((s) => s.updateEditorSettings)
  const wrap = kind === 'code' ? codeWrap : mdWrap
  const name = filePath.split('/').pop() || filePath

  // A key moved the mode: keep the keyboard in the tab (Read has no editor).
  const refocus = useRef(false)
  useEffect(() => {
    if (!refocus.current) return
    refocus.current = false
    root.current?.querySelector<HTMLElement>('.cm-content')?.focus()
  }, [mode, root])

  const onKeyDownCapture = (e: React.KeyboardEvent): void => {
    const mod = (e.metaKey || e.ctrlKey) && !e.altKey
    if (mod && kind === 'markdown' && e.key.toLowerCase() === 'e') {
      e.preventDefault()
      e.stopPropagation()
      const next = modeForKey(mode, e, ro, large)
      if (next) {
        refocus.current = true
        setPicked(next)
      }
    } else if (mod && e.key.toLowerCase() === 's' && mode === 'read') {
      e.preventDefault()
      e.stopPropagation()
      onSave?.()
    }
  }

  // The status line.
  const saveWord = saving ? 'Saving' : dirty ? 'Not saved' : 'Saved'
  const save: StatusItem[] = ro ? [] : [{ id: 'save', node: <SaveWord word={saveWord} /> }]
  const pos: StatusItem = { id: 'pos', node: `Ln ${cursor.line}, Col ${cursor.col}` }
  let items: StatusItem[]
  if (kind === 'code') items = [pos, ...save]
  else {
    const n = countWords(text)
    items = [{ id: 'words', node: `${n} ${n === 1 ? 'word' : 'words'}` }, ...(n ? [{ id: 'read', node: `${readingMinutes(n)} min read` }] : []), ...save, ...(mode === 'source' ? [pos] : [])]
  }
  const fullToggle: StatusToggle = { id: 'full', label: 'Full width', on: full, onToggle: () => setFull(!full) }
  const wrapToggle: StatusToggle = { id: 'wrap', label: 'Wrap', on: wrap, onToggle: () => (kind === 'code' ? void updateEditor({ wordWrap: !codeWrap }) : setMdWrap(!mdWrap)) }
  const toggles = kind === 'code' || mode === 'source' ? [wrapToggle] : mode === 'read' ? [fullToggle] : [fullToggle, wrapToggle]
  const facts: StatusItem[] =
    kind === 'code'
      ? [
          { id: 'lang', node: getLanguageName(filePath) },
          { id: 'enc', node: 'UTF-8' },
          { id: 'eol', node: lineEnding(text) },
        ]
      : []

  const modeControl =
    kind === 'markdown' ? (
      <Segmented<EditorMode>
        aria-label="Mode"
        className="ed-modes"
        value={mode}
        onChange={setPicked}
        options={MODE_OPTIONS.map((o) => {
          const off = o.value === 'source' ? null : large ? TOO_LARGE_REASON : o.value === 'write' ? readOnly : null
          return off ? { ...o, offReason: off } : o
        })}
      />
    ) : null

  // One element in every mode, so switching keeps the editor (and its undo history).
  const body = (
    <div data-testid={mode === 'read' ? 'editor-read' : undefined} aria-label={mode === 'read' ? `${name}, reading view` : undefined} className="min-h-0 flex-1 overflow-hidden">
      {renderEditor({ mode, readOnly: ro, lineNumbers: mode === 'source' ? lineNumbers : false, wrap: mode === 'read' || wrap, extensions })}
    </div>
  )

  return (
    // data-k2-line: the mode control measures its room against the tab, not its own wrapper (ui/segmented-fit.ts).
    <div ref={root} data-testid="editor-tab" data-k2-line="" data-kind={kind} data-mode={mode} data-full={full ? '' : undefined} className="ed-tab flex h-full min-h-0 flex-col" onKeyDownCapture={onKeyDownCapture}>
      <style>{EDITOR_CSS + WRITE_CSS}</style>
      <LiveTabFrame
        header={<LiveHeader fileName={name} fileTitle={filePath} fileGlyph={<SetiFileIcon name={name} size={16} />} sentence={null} mode={modeControl} />}
        notice={notice}
        hero={<div className="ed-body flex min-h-0 flex-1 flex-col">{body}</div>}
        status={<EditorStatusLine items={items} toggles={toggles} facts={facts} narrow={narrow} />}
        panel={null}
        panelOpen={false}
        panelMode="reflow"
        narrow={narrow}
        sample={false}
        onClosePanel={() => {}}
      />
    </div>
  )
}
