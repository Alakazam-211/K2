// The outline (build-editors.md E3; editors-notes.md §2 Chrome): the file's
// headings in the side panel, indented by level, each a button that jumps to
// its heading. The heading the top of the view is in is marked
// (aria-current), and the mark follows as you scroll. The headings come from
// the text (lib/editor/outline), with GitHub's slugs, so Write, Read, Source
// and a live doc agree.

import { useEffect, useMemo, useState } from 'react'
import { EditorSelection } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import type { FrameView } from '@/lib/editor/frame-view'
import { cx } from '@/components/ui'
import { headingAt, outlineOf, type OutlineItem } from '@/lib/editor/outline/outline'
import { PanelTabs, type PanelTabsValue } from './PanelTabs'

/** The heading index at the top of the view (a little below the edge), -1 above the first. */
function topHeading(view: EditorView, items: readonly OutlineItem[]): number {
  if (!items.length) return -1
  const top = view.scrollDOM.getBoundingClientRect().top
  const block = view.lineBlockAtHeight(top - view.documentTop + 24)
  return headingAt(items, view.state.doc.lineAt(Math.min(block.from, view.state.doc.length)).number)
}

export function jumpToHeading(view: EditorView, item: OutlineItem): void {
  const line = view.state.doc.line(Math.min(item.line, view.state.doc.lines))
  view.dispatch({ selection: EditorSelection.cursor(line.from), effects: EditorView.scrollIntoView(line.from, { y: 'start', yMargin: 24 }), userEvent: 'select.outline' })
  view.focus()
}

/** `store`: the frame's; `version` redraws the panel as the view changes. Never
 *  the view itself as a prop (see FindBarProps.store). */
export function OutlinePanel({ store, version, tabs }: { store: FrameView; version: number; tabs: PanelTabsValue }): React.JSX.Element {
  const view = store.get()
  const doc = view?.state.doc ?? null
  const items = useMemo(() => (doc ? outlineOf(doc) : []), [doc])
  const [current, setCurrent] = useState(-1)
  useEffect(() => {
    if (!view) return
    let frame = 0
    const read = (): void => {
      frame = 0
      setCurrent(topHeading(view, items))
    }
    read()
    const onScroll = (): void => {
      if (!frame) frame = requestAnimationFrame(read)
    }
    view.scrollDOM.addEventListener('scroll', onScroll, { passive: true })
    return () => {
      view.scrollDOM.removeEventListener('scroll', onScroll)
      if (frame) cancelAnimationFrame(frame)
    }
  }, [view, items, version])
  const min = items.reduce((m, h) => Math.min(m, h.level), 6)
  return (
    <div data-testid="ed-outline-panel" className="flex min-h-full flex-col">
      <PanelTabs {...tabs} />
      <div role="tabpanel" aria-labelledby="ed-panel-tab-outline" data-testid="ed-outline" className="pb-3">
        {items.length === 0 ? <p className="type-hint m-0 px-3.5 py-2 text-[var(--color-text-secondary)]">Headings show here as you add them.</p> : null}
        {items.map((h, i) => (
          <button
            key={`${h.slug}:${h.line}`}
            type="button"
            data-k2-focus=""
            data-slug={h.slug}
            aria-current={i === current ? 'location' : undefined}
            title={`Line ${h.line}`}
            onClick={() => view && jumpToHeading(view, h)}
            style={{ '--oi': h.level - min } as React.CSSProperties}
            className={cx('ed-orow lv-row-btn no-drag type-row', h.level === min ? 'ed-orow-top' : '')}
          >
            <span className="min-w-0 flex-1 [overflow-wrap:anywhere]">{h.text || 'Untitled heading'}</span>
          </button>
        ))}
      </div>
    </div>
  )
}
