// The editor's side-panel tabs (build-editors.md E3; editors-notes.md §2 Chrome):
// "Outline" for a markdown file, then "Live 3" while someone else is in it. A
// WAI-ARIA tab strip: ← → move and choose. The live panel (live/LivePanel)
// shows the same strip through PanelTabsContext, so both tabs look and act
// alike whichever is open.

import { createContext } from 'react'
import { cx } from '@/components/ui'
import { ToolGlyph } from '../tools/glyphs'

export interface PanelTab {
  id: 'outline' | 'live'
  label: string
  /** After the label ("3"), quiet. */
  count?: React.ReactNode
}

export interface PanelTabsValue {
  tabs: PanelTab[]
  selected: PanelTab['id']
  onSelect: (id: PanelTab['id']) => void
}

/** Set by the frame for a live markdown file: Outline and Live share one strip. */
export const PanelTabsContext = createContext<PanelTabsValue | null>(null)

export function PanelTabs({ tabs, selected, onSelect }: PanelTabsValue): React.JSX.Element {
  const go = (e: React.KeyboardEvent, i: number): void => {
    const n = tabs.length
    const next = e.key === 'ArrowRight' ? (i + 1) % n : e.key === 'ArrowLeft' ? (i - 1 + n) % n : -1
    if (next < 0) return
    e.preventDefault()
    onSelect(tabs[next].id)
    ;(e.currentTarget.parentElement?.querySelectorAll<HTMLElement>('[role="tab"]')[next])?.focus()
  }
  return (
    <div role="tablist" aria-label="Side panel" className="flex flex-none items-center gap-1 px-3 pb-1 pt-2">
      {tabs.map((t, i) => {
        const on = t.id === selected
        return (
          <button
            key={t.id}
            type="button"
            role="tab"
            id={`ed-panel-tab-${t.id}`}
            aria-selected={on}
            tabIndex={on ? 0 : -1}
            data-k2-focus=""
            data-k2-chamfer="control"
            onClick={() => onSelect(t.id)}
            onKeyDown={(e) => go(e, i)}
            className={cx('ed-panel-tab no-drag type-row inline-flex cursor-pointer items-center gap-1.5 rounded-[var(--radius-pill)] border px-3 py-1', on ? 'border-[var(--color-control-edge)] font-semibold text-[var(--color-text-primary)]' : 'ed-panel-tab-off border-transparent text-[var(--color-text-secondary)]')}
          >
            {t.id === 'outline' ? <ToolGlyph name="outline" size={13} /> : null}
            {t.label}
            {t.count !== undefined ? t.count : null}
          </button>
        )
      })}
    </div>
  )
}
