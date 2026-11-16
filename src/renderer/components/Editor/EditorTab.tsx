// The new file editors (build-editors.md E1b), behind their preview switches
// (lib/editor/preview-switch.ts): two editors on one shared frame
// (EditorFrame). MarkdownEditor has Write | Read | Source, the rendered page
// (E2), words and read time; CodeEditorTab has no Write or Read: the text, the
// caret, the language, encoding and line endings, Wrap, and Preview | Source
// for a file with its own preview. The file tab picks one by the file's kind.

import type { EditorKind } from '@/lib/editor/preview-switch'
import { CodeEditorTab } from './CodeEditorTab'
import type { EditorProps } from './EditorFrame'
import { MarkdownEditor } from './MarkdownEditor'

export { EDITOR_NARROW_BELOW, type EditorMountOptions } from './EditorFrame'
export { TOO_LARGE_REASON } from './MarkdownEditor'

export interface EditorTabProps extends EditorProps {
  kind: EditorKind
}

export function EditorTab({ kind, ...p }: EditorTabProps): React.JSX.Element {
  return kind === 'markdown' ? <MarkdownEditor {...p} /> : <CodeEditorTab {...p} />
}
