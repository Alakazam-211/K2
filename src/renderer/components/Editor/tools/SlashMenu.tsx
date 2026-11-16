// The / menu's list (build-editors.md E3; editors-notes.md §2): "Insert a
// block. Keep typing to filter." over the blocks still matching, each with
// its tile, its name and the markdown it types. The editor keeps the keyboard
// (↑ ↓ Enter Esc are the view's, lib/editor/slash) and points at the chosen
// row with aria-activedescendant; a press picks a row. Glass draws it on the
// frost float, Terminal cut.

import { forwardRef, useLayoutEffect, useRef } from 'react'
import type { EditorView } from '@codemirror/view'
import { cx, floatMaterial } from '@/components/ui'
import { filterBlocks, pickBlock, type SlashState } from '@/lib/editor/slash/slash'
import { ToolGlyph } from './glyphs'

export const SlashMenu = forwardRef<HTMLDivElement, { view: EditorView; state: SlashState; id: string; style: React.CSSProperties }>(function SlashMenu({ view, state, id, style }, ref) {
  const blocks = filterBlocks(state.query)
  const list = useRef<HTMLDivElement>(null)
  // A capped list keeps the chosen row in view.
  useLayoutEffect(() => {
    const on = list.current?.querySelector<HTMLElement>('[aria-selected="true"]')
    if (on && typeof on.scrollIntoView === 'function') on.scrollIntoView({ block: 'nearest' })
  }, [state.active, state.query])
  return (
    <div ref={ref} data-testid="ed-slash" data-k2-chamfer="surface" className="ed-slash no-drag" style={style} {...floatMaterial('menu', 'frost')} onMouseDown={(e) => e.preventDefault()}>
      <p className="ed-slash-head">{blocks.length ? 'Type to filter' : 'No blocks match'}</p>
      <div ref={list} role="listbox" id={`${id}-menu`} aria-label="Insert a block" className="ed-slash-list">
        {blocks.map((b, i) => (
          <div
            key={b.id}
            id={`${id}-${b.id}`}
            role="option"
            aria-selected={i === state.active}
            className={cx('ed-slash-opt', i === state.active ? 'ed-slash-on' : '')}
            onMouseDown={(e) => {
              e.preventDefault()
              pickBlock(view, b.id)
            }}
          >
            <span aria-hidden="true" data-k2-chamfer="chip" className="ed-slash-tile">
              <ToolGlyph name={b.id} size={14} />
            </span>
            <span className="ed-slash-name">{b.label}</span>
            <kbd className="ed-slash-hint">{b.hint}</kbd>
          </div>
        ))}
      </div>
    </div>
  )
})
