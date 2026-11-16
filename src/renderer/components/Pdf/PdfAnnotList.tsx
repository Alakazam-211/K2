// The Annotations tab: the PDF's comments, markups and stamps, grouped by
// page, each row its author, date, kind and text. A row jumps to its page
// and opens its popover. Filters by kind sit on top ("All 7", "Comment 2"…).
// Rows are keyed by /NM, else page and index (lib/pdf/annotations.ts), so
// P3's live markup can add K2's attribution without reshaping the list.

import { useState } from 'react'
import { cx } from '@/components/ui'
import { applyFilter, groupByPage, subtypeFilters, type PdfAnnot } from '@/lib/pdf/annotations'
import { formatAnnotDate } from './annot-format'

export function PdfAnnotList({ annots, selectedKey, onOpen }: { annots: readonly PdfAnnot[] | null; selectedKey: string | null; onOpen: (a: PdfAnnot) => void }): React.JSX.Element {
  const [filter, setFilter] = useState('all')
  if (annots === null) return <p className="pdf-empty type-row m-0">Reading the annotations…</p>
  if (annots.length === 0) return <p className="pdf-empty type-row m-0">This PDF has no annotations.</p>
  const filters = subtypeFilters(annots)
  const on = filters.some((f) => f.id === filter) ? filter : 'all'
  const groups = groupByPage(applyFilter(annots, on))
  const move = (e: React.KeyboardEvent, i: number): void => {
    const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0
    if (!step) return
    e.preventDefault()
    const n = (i + step + filters.length) % filters.length
    setFilter(filters[n].id)
    ;(e.currentTarget.parentElement?.children[n] as HTMLElement | undefined)?.focus()
  }
  return (
    <div data-testid="pdf-annots">
      <div role="radiogroup" aria-label="Show" className="pdf-filters">
        {filters.map((f, i) => (
          <button
            key={f.id}
            type="button"
            role="radio"
            aria-checked={f.id === on}
            tabIndex={f.id === on ? 0 : -1}
            data-filter={f.id}
            data-k2-focus=""
            data-k2-chamfer="control"
            className="ed-chip-btn pdf-chip type-hint"
            onClick={() => setFilter(f.id)}
            onKeyDown={(e) => move(e, i)}
          >
            {f.label} <span className="pdf-chip-count lv-num">{f.count}</span>
          </button>
        ))}
      </div>
      {groups.map((g) => (
        <section key={g.page} aria-labelledby={`pdf-annots-p${g.page}`}>
          <h4 id={`pdf-annots-p${g.page}`} className="pdf-group-head type-hint m-0">
            Page {g.page}
          </h4>
          <ul className="m-0 list-none p-0">
            {g.items.map((a) => (
              <li key={a.key}>
                <button type="button" data-testid="pdf-annot-row" data-key={a.key} data-k2-focus="" aria-current={a.key === selectedKey || undefined} className={cx('lv-row-btn pdf-annot-row no-drag', a.key === selectedKey ? 'pdf-row-on' : '')} onClick={() => onOpen(a)}>
                  <span className="pdf-annot-line type-row">
                    <b className="font-semibold">{a.author}</b>
                    {a.date ? (
                      <>
                        <span aria-hidden="true" className="pdf-dot">
                          ·
                        </span>
                        <time dateTime={a.date.toISOString()} className="pdf-quiet">
                          {formatAnnotDate(a.date)}
                        </time>
                      </>
                    ) : null}
                  </span>
                  <span className="pdf-annot-line type-hint">
                    <span className="pdf-kind">{a.label}</span>
                    {a.text ? <span className="pdf-annot-text">{a.text}</span> : null}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  )
}
