// The find bar's CodeMirror half (build-editors.md E3): CodeMirror marks the
// matches (in the find tokens, lib/editor/theme.ts) only while its search
// panel is open, so the bar opens that panel, an empty hidden box
// (lib/editor/frame-view.ts), and sets the query on it; closing the bar
// closes it.

import type { EditorView } from '@codemirror/view'
import { closeSearchPanel, openSearchPanel, searchPanelOpen, setSearchQuery } from '@codemirror/search'
import { toQuery, type FindSpec } from './find'

export function showMatches(view: EditorView, spec: FindSpec): void {
  if (!searchPanelOpen(view.state)) openSearchPanel(view)
  view.dispatch({ effects: setSearchQuery.of(toQuery(spec)) })
}

export function hideMatches(view: EditorView): void {
  if (searchPanelOpen(view.state)) closeSearchPanel(view)
}

export { searchPanelOpen }
