// The new code editor ("New code editor", build-editors.md E1b), on the shared
// EditorFrame: every text file that isn't markdown (code, config, plain text,
// logs). No mode control: the caller's CodeMirror, with line numbers as Code
// settings say. The status line: the caret and the save word; Wrap (Code
// settings' word wrap); the language, encoding and line endings.
//
// A file with its own preview (CSV/TSV, HTML, Mermaid, diagram and DXF
// sources) gets Preview | Source: Preview is today's viewer for it, unchanged
// until E6 restyles it, and the file opens in it as it does today. ⌘E toggles
// the two; ⇧⌘E opens Source.

import { useState } from 'react'
import type { Extension } from '@codemirror/state'
import { Segmented } from '@/components/ui'
import { getLanguageName } from '@/components/FileViewerPane/CodeEditor'
import { lineEnding } from '@/lib/editor/words'
import { useSettingsStore } from '@/stores/settings'
import { EditorFrame, caretItem, saveItems, saveKey, useEditorFrame, type EditorProps } from './EditorFrame'
import type { StatusItem, StatusToggle } from './EditorStatusLine'

export type CodeMode = 'preview' | 'source'

const NONE: Extension = []

const MODE_OPTIONS: { value: CodeMode; label: string }[] = [
  { value: 'preview', label: 'Preview' },
  { value: 'source', label: 'Source' },
]

export function CodeEditorTab(p: EditorProps): React.JSX.Element {
  const { filePath, text, renderPreview } = p
  const [picked, setPicked] = useState<CodeMode>('preview')
  const mode: CodeMode = renderPreview ? picked : 'source'
  const frame = useEditorFrame(p.width, mode)
  const wrap = useSettingsStore((s) => s.editor.wordWrap)
  const lineNumbers = useSettingsStore((s) => s.editor.lineNumbers)
  const updateEditor = useSettingsStore((s) => s.updateEditorSettings)

  const onKeyDownCapture = (e: React.KeyboardEvent): void => {
    if (renderPreview && (e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === 'e') {
      e.preventDefault()
      e.stopPropagation()
      const next: CodeMode = e.shiftKey || mode === 'preview' ? 'source' : 'preview'
      if (next === 'source') frame.refocusAfterModeChange()
      setPicked(next)
    } else if (mode === 'preview') saveKey(e, p.onSave)
  }

  const items: StatusItem[] = [...(mode === 'source' ? [caretItem(p.cursor)] : []), ...saveItems(p)]
  const toggles: StatusToggle[] = mode === 'source' ? [{ id: 'wrap', label: 'Wrap', on: wrap, onToggle: () => void updateEditor({ wordWrap: !wrap }) }] : []
  const facts: StatusItem[] = [
    { id: 'lang', node: getLanguageName(filePath) },
    { id: 'enc', node: 'UTF-8' },
    { id: 'eol', node: lineEnding(text) },
  ]
  const modeControl = renderPreview ? <Segmented<CodeMode> aria-label="Mode" className="ed-modes" value={mode} onChange={setPicked} options={MODE_OPTIONS} /> : null

  const body =
    mode === 'preview' && renderPreview ? (
      <div data-testid="editor-preview" className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {renderPreview()}
      </div>
    ) : (
      <div className="min-h-0 flex-1 overflow-hidden">
        {p.renderEditor({ mode: 'source', readOnly: p.readOnly !== null, lineNumbers, wrap, extensions: NONE, frame: frame.extensionFor(false) })}
      </div>
    )

  return <EditorFrame frame={frame} filePath={filePath} kind="code" mode={mode} modeControl={modeControl} readOnly={p.readOnly} notice={p.notice} body={body} status={{ items, toggles, facts }} onKeyDownCapture={onKeyDownCapture} />
}
