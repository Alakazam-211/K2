// CodeEditor's live-reconfigurable settings, one set per editor view
// (build-editors.md E1a). They used to be module-level, shared by every
// editor in the window, so one view's reconfigure could reach a compartment
// another view was built with (P12).

import { Compartment } from '@codemirror/state'

const NAMES = [
  'wrap', 'tabSize', 'whitespace', 'indentGuides', 'fold', 'autocomplete', 'lineNumbers', 'activeLine',
  'bracket', 'fontSize', 'fontFamily', 'cursor', 'theme', 'scrollPastEnd', 'vim', 'stickyScroll',
  'minimap', 'gitGutter', 'highlight',
] as const

export type EditorCompartments = Record<(typeof NAMES)[number], Compartment>

export function createEditorCompartments(): EditorCompartments {
  return Object.fromEntries(NAMES.map((n) => [n, new Compartment()])) as EditorCompartments
}
