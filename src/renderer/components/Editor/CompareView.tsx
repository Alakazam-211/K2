// Compare after a merge (notes v3.1, v3.2): "Before external edit" (your
// text as it was) against now, in the editor's place, read only. Rows are
// aligned by a line diff (@codemirror/merge), so only the lines that differ
// are marked: − on the before side, + on now. Unchanged runs fold, two lines
// of context kept. A narrow tab shows one column (unified).

import { useEffect, useRef } from 'react'
import { EditorState, type Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { MergeView, unifiedMergeView } from '@codemirror/merge'
import { useTabWidth } from '@/components/LiveView/shell/useLiveShell'
import { getHighlightExtension, getThemeExtension, FOLLOW_APP_THEME_ID } from '@/lib/editor/theme'

const base = (): Extension[] => [
  EditorState.readOnly.of(true),
  // The folds read as a link (notes v3.2): "Show 22 unchanged lines".
  EditorState.phrases.of({ '$ unchanged lines': 'Show $ unchanged lines' }),
  EditorView.editable.of(false),
  EditorView.lineWrapping,
  getThemeExtension(FOLLOW_APP_THEME_ID),
  getHighlightExtension(FOLLOW_APP_THEME_ID),
]

const COLLAPSE = { margin: 2, minSize: 4 }

/** Narrower than this, one column. */
const ONE_COLUMN_BELOW = 900

export function CompareView({ before, now, width }: { before: string; now: string; width?: number }): React.JSX.Element {
  const host = useRef<HTMLDivElement>(null)
  const [outer, w] = useTabWidth(width)
  const narrow = w < ONE_COLUMN_BELOW
  useEffect(() => {
    const parent = host.current
    if (!parent) return
    if (narrow) {
      const view = new EditorView({ parent, doc: now, extensions: [...base(), unifiedMergeView({ original: before, mergeControls: false, collapseUnchanged: COLLAPSE })] })
      return () => view.destroy()
    }
    const view = new MergeView({ parent, a: { doc: before, extensions: base() }, b: { doc: now, extensions: base() }, collapseUnchanged: COLLAPSE, gutter: true })
    return () => view.destroy()
  }, [before, now, narrow])

  return (
    <div ref={outer} data-testid="ed-compare" data-narrow={narrow ? '' : undefined} className="ed-compare flex min-h-0 flex-1 flex-col">
      <div className="ed-compare-heads">
        <span className="ed-compare-head" data-side="before">
          <b>− Before external edit</b>, your text as it was
        </span>
        <span className="ed-compare-head" data-side="now">
          <b>+ Now</b>, with the save merged in
        </span>
      </div>
      <div ref={host} className="ed-compare-body min-h-0 flex-1 overflow-auto" aria-label="Before external edit, compared with now" role="region" />
    </div>
  )
}
