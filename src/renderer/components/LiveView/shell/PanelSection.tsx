// One section of the live tab's side panel: a heading that folds it (name
// and a quiet count or word), and its rows. The tab orders its sections by
// one PANEL array (scene-shell.ts, timeline-shell.ts), so the order is one
// change. Whole rows only; the panel scrolls with fades (ScrollEdgeFrame,
// in LiveTabFrame).

import { useState } from 'react'
import { cx } from '@/components/ui'
import { ChevronGlyph } from './glyphs'

export function PanelSection({
  id,
  title,
  count,
  countTestId,
  defaultOpen = true,
  action,
  children,
}: {
  id: string
  title: string
  /** A number, or a word ("as you last saw"). */
  count?: number | string | null
  countTestId?: string
  defaultOpen?: boolean
  /** A control at the heading's end, beside the fold ("Live edits"). */
  action?: React.ReactNode
  children: React.ReactNode
}): React.JSX.Element {
  const [open, setOpen] = useState(defaultOpen)
  const bodyId = `lv-sec-${id}`
  return (
    <section data-testid={`lv-section-${id}`} className="lv-section">
      <h3 className={cx('m-0', action ? 'flex items-center pr-2.5' : '')}>
        <button
          type="button"
          data-k2-focus=""
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={() => setOpen((o) => !o)}
          className="lv-row-btn no-drag flex w-full cursor-pointer items-center gap-1.5 rounded-[var(--radius-selector)] px-3.5 pb-1 pt-2.5 text-left"
        >
          <span className="type-row font-semibold text-[var(--color-text-primary)]">{title}</span>
          {count != null ? (
            <span data-testid={countTestId} className="type-hint text-[var(--color-text-secondary)]">
              {count}
            </span>
          ) : null}
          <span className="flex-1" />
          <span aria-hidden="true" className={cx('inline-flex text-[var(--color-text-secondary)] transition-transform', open ? '' : '-rotate-90')}>
            <ChevronGlyph size={13} />
          </span>
        </button>
        {action}
      </h3>
      {open ? (
        <div id={bodyId} className="pb-1.5">
          {children}
        </div>
      ) : null}
    </section>
  )
}
