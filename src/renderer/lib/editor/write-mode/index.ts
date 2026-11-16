// Write and Read for markdown files (build-editors.md E2): one CodeMirror
// extension, so a plain file passes it to CodeEditor and a live file can pass
// the same bundle to mountLiveEditor({extensions}) later (E4).
//
// - Write: you edit the rendered page. Only the lines the caret or a
//   selection touches show their marks, in the muted ink (./reveal).
// - Read: the same rendered view, read only, with no caret and no reveal;
//   a plain click opens a link.
// - Source is CodeEditor without this extension.
//
// Files over 1 MB or 20,000 lines open in Source (writeModeFits).

import { EditorState, type Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { gfmMarkdown } from './language'
import { readReveal, revealExtension } from './reveal'
import { blockField } from './blocks'
import { inlineDecorations } from './inline'
import { imageCache } from './widgets'
import { openLink, writeLinks, type WriteLinks } from './links'

export type { WriteLinks } from './links'
export { toggleTask } from './tasks'

/** Above these, Write and Read are off and the file opens in Source (§6). */
export const WRITE_MAX_BYTES = 1024 * 1024
export const WRITE_MAX_LINES = 20_000

/** Can this text open in Write and Read? */
export function writeModeFits(text: string): boolean {
  let lines = 1
  for (let i = text.indexOf('\n'); i !== -1; i = text.indexOf('\n', i + 1)) if (++lines > WRITE_MAX_LINES) return false
  if (text.length * 3 <= WRITE_MAX_BYTES) return true
  return new TextEncoder().encode(text).length <= WRITE_MAX_BYTES
}

/** ⌘-click (Ctrl-click) opens a link in Write; any click does in Read. */
const linkClicks = (read: boolean): Extension =>
  EditorView.domEventHandlers({
    mousedown(e, view) {
      const a = (e.target as Element | null)?.closest?.<HTMLElement>('.cm-md-link[data-href]')
      if (!a || e.button !== 0 || !(read || e.metaKey || e.ctrlKey)) return false
      e.preventDefault()
      openLink(view.state.facet(writeLinks), a.dataset.href ?? '')
      return true
    },
  })

export function writeMode(mode: 'write' | 'read', links: WriteLinks): Extension {
  const read = mode === 'read'
  return [
    gfmMarkdown(),
    writeLinks.of(links),
    read
      ? [
          readReveal,
          EditorState.readOnly.of(true),
          EditorView.editable.of(false),
          // Focusable, so the keys scroll and select, but no caret.
          EditorView.contentAttributes.of({ tabindex: '0', 'aria-readonly': 'true' }),
        ]
      : revealExtension(),
    blockField,
    inlineDecorations(read),
    imageCache,
    linkClicks(read),
    EditorView.editorAttributes.of({ class: `cm-md cm-md-${mode}` }),
  ]
}
