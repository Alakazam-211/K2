// The editor frame's line to whichever CodeMirror it has mounted
// (build-editors.md E3). The frame's tools (find, go to line, the outline,
// the formatting toolbar, the / menu) sit outside the text, in React; the
// view is made deep inside (CodeEditor for a plain file, the Live editor for
// a live one). Each editor mode hands the view `frameExtension(...)`: a plugin
// that tells the frame's store which view is there and when it changed, the
// find bar's CodeMirror half (search state, with CodeMirror's own panel never
// shown), and in Write the / menu and the format keys.

import type { Extension } from '@codemirror/state'
import { EditorView, ViewPlugin, keymap } from '@codemirror/view'
import { search } from '@codemirror/search'
import { FORMAT_IDS, FORMAT_KEYS, formatSpec } from './md-commands'
import { slashExtension } from './slash/slash'
import { findMarks } from './find/find-view'

export interface FrameView {
  /** The mounted view, or null. */
  get(): EditorView | null
  /** Changes on every update of the view (and when it comes or goes). */
  version(): number
  subscribe(fn: () => void): () => void
  /** The / menu's id base (its listbox and options), unique to this frame. */
  readonly slashId: string
}

interface Store extends FrameView {
  attach(view: EditorView): void
  detach(view: EditorView): void
  bump(): void
}

let ids = 0

export function createFrameView(): Store {
  let view: EditorView | null = null
  let v = 0
  const subs = new Set<() => void>()
  const bump = (): void => {
    v++
    for (const fn of subs) fn()
  }
  return {
    slashId: `ed-slash-${++ids}`,
    get: () => view,
    version: () => v,
    subscribe(fn) {
      subs.add(fn)
      return () => void subs.delete(fn)
    },
    attach(next) {
      view = next
      bump()
    },
    detach(old) {
      if (view !== old) return
      view = null
      bump()
    },
    bump,
  }
}

/** The find bar keeps its query in CodeMirror's search state; CodeMirror's own panel (never opened) is an empty, hidden box. */
export const findPanelExtension: Extension = search({
  createPanel: () => {
    const dom = document.createElement('div')
    dom.className = 'ed-find-proxy'
    dom.hidden = true
    return { dom, top: true }
  },
})

/** Write's format keys (⌘B, ⌘I, ⇧⌘C, ⌘K, ⌥⌘2, ⇧⌘8); they stop here, so ⌘K never reaches the palette. */
const formatKeys: Extension = keymap.of(
  FORMAT_IDS.map((id) => ({
    key: FORMAT_KEYS[id].key,
    stopPropagation: true,
    run: (view: EditorView) => {
      const spec = formatSpec(view.state, id)
      if (spec) view.dispatch(spec)
      return true
    },
  })),
)

/** One mode's extension for the frame's view. `write`: the / menu and the format keys too. */
export function frameExtension(store: FrameView, o: { write: boolean }): Extension {
  const s = store as Store
  const plugin = ViewPlugin.define((view) => {
    s.attach(view)
    return {
      update: () => s.bump(),
      destroy: () => s.detach(view),
    }
  })
  return [findMarks, plugin, findPanelExtension, o.write ? [slashExtension(store.slashId), formatKeys] : []]
}
