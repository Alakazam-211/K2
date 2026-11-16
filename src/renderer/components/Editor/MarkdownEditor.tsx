// The new markdown editor ("New markdown editor", build-editors.md E1b and
// E2), on the shared EditorFrame: Write | Read | Source. Every mode mounts the
// caller's CodeMirror: Write and Read with the write-mode extension (the
// rendered page; Read is it, read only, with no caret), Source with none. ⌘E
// toggles Write and Read, ⇧⌘E opens Source. A file over 1 MB or 20,000 lines
// is Source only. The status line: words, read time, the save word, and the
// caret in Source; Full width and Wrap. The reading column is the Thread's
// text width (lib/editor/measure.ts).

import { useContext, useEffect, useState } from 'react'
import { Segmented } from '@/components/ui'
import { useLocalFlag } from '@/lib/editor/local-flag'
import { initialMode, keepModeForHandover, modeForKey, takeHandoverMode, usableMode, type EditorMode } from '@/lib/editor/modes'
import { countWords, readingMinutes } from '@/lib/editor/words'
import { writeModeFits } from '@/lib/editor/write-mode'
import { useSettingsStore } from '@/stores/settings'
import { useThreadTextCapPx } from '@/components/SessionView/useThreadTextCap'
import { editorMeasure } from '@/lib/editor/measure'
import { FrameLiveContext } from './frameLive'
import { useWriteMode } from './useWriteMode'
import { EditorFrame, caretItem, fileNameOf, saveItems, saveKey, useEditorFrame, type EditorProps } from './EditorFrame'
import type { StatusItem, StatusToggle } from './EditorStatusLine'

const FULL_WIDTH_KEY = 'k2.editor.fullWidth.v1'
const MARKDOWN_WRAP_KEY = 'k2.editor.markdownWrap.v1'
export const TOO_LARGE_REASON = 'This file is too large to show formatted (over 1 MB or 20,000 lines)'

const MODE_OPTIONS: { value: EditorMode; label: string }[] = [
  { value: 'write', label: 'Write' },
  { value: 'read', label: 'Read' },
  { value: 'source', label: 'Source' },
]

export function MarkdownEditor(p: EditorProps): React.JSX.Element {
  const { filePath, text, readOnly } = p
  const ro = readOnly !== null
  // Decided once per file: typing past the limit doesn't throw you out of Write.
  const [large] = useState(() => !writeModeFits(text))
  // A live file keeps its mode for the plain editor that takes over if live editing turns off.
  const live = useContext(FrameLiveContext) !== null
  const [picked, setPicked] = useState<EditorMode>(() => (live ? undefined : takeHandoverMode(filePath)) ?? initialMode('markdown', ro, large))
  useEffect(() => void (live && keepModeForHandover(filePath, picked)), [live, filePath, picked])
  const mode = usableMode(picked, ro, large)
  const frame = useEditorFrame(p.width, mode)
  const extensions = useWriteMode(mode, filePath)
  const [full, setFull] = useLocalFlag(FULL_WIDTH_KEY)
  const [wrap, setWrap] = useLocalFlag(MARKDOWN_WRAP_KEY, true)
  const lineNumbers = useSettingsStore((s) => s.editor.lineNumbers)
  // The reading column is the Thread's text width (the same setting), wider at Full width.
  const measure = editorMeasure(useThreadTextCapPx(), full)
  const name = fileNameOf(filePath)

  const onKeyDownCapture = (e: React.KeyboardEvent): void => {
    if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === 'e') {
      e.preventDefault()
      e.stopPropagation()
      const next = modeForKey(mode, e, ro, large)
      if (next) {
        frame.refocusAfterModeChange()
        setPicked(next)
      }
    } else if (mode === 'read') saveKey(e, p.onSave)
  }

  const n = countWords(text)
  const items: StatusItem[] = [{ id: 'words', node: `${n} ${n === 1 ? 'word' : 'words'}` }, ...(n ? [{ id: 'read', node: `${readingMinutes(n)} min read` }] : []), ...saveItems(p), ...(mode === 'source' ? [caretItem(p.cursor)] : [])]
  const fullToggle: StatusToggle = { id: 'full', label: 'Full width', on: full, onToggle: () => setFull(!full) }
  const wrapToggle: StatusToggle = { id: 'wrap', label: 'Wrap', on: wrap, onToggle: () => setWrap(!wrap) }
  const toggles = mode === 'source' ? [wrapToggle] : mode === 'read' ? [fullToggle] : [fullToggle, wrapToggle]

  const modeControl = (
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
  )

  // One element in every mode, so switching keeps the editor (and its undo history).
  const body = (
    <div data-testid={mode === 'read' ? 'editor-read' : undefined} aria-label={mode === 'read' ? `${name}, reading view` : undefined} className="min-h-0 flex-1 overflow-hidden">
      {p.renderEditor({ mode, readOnly: ro, lineNumbers: mode === 'source' ? lineNumbers : false, wrap: mode === 'read' || wrap, extensions, frame: frame.extensionFor(mode === 'write') })}
    </div>
  )

  return <EditorFrame frame={frame} filePath={filePath} kind="markdown" mode={mode} full={full} style={{ '--ed-measure': measure } as React.CSSProperties} modeControl={modeControl} readOnly={readOnly} notice={p.notice} body={body} status={{ items, toggles, facts: [] }} onKeyDownCapture={onKeyDownCapture} />
}
