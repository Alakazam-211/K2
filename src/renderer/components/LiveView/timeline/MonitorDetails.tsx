// The monitor's details (the ⓘ): K2's float surface (Glass frosts it, as a
// floating popover; Terminal cuts it) over the monitor's top-right, as tall
// as its rows up to the monitor, then it scrolls with a "More below" cue; in a
// narrow window or a short monitor, a sheet over the tab. Esc or Close
// shuts it and puts focus back on the ⓘ. It carries the `absolute` class
// itself: Veil's frost sets any frost box without it back in the flow.

import { useEffect, useRef, useState } from 'react'
import { floatMaterial } from '@/components/ui'
import type { DetailRow } from '@/lib/live-view/timeline/monitor-details'
import { CloseGlyph } from '../shell/glyphs'
import { DownGlyph } from './glyphs'

export function MonitorDetails({ rows, sheet, maxH, onClose }: { rows: DetailRow[]; sheet: boolean; maxH: number; onClose: () => void }): React.JSX.Element {
  const close = useRef<HTMLButtonElement>(null)
  const scroll = useRef<HTMLDivElement>(null)
  const [more, setMore] = useState(false)
  useEffect(() => close.current?.focus(), [])
  useEffect(() => {
    const s = scroll.current
    if (!s) return
    const read = (): void => setMore(s.scrollHeight - s.clientHeight - s.scrollTop > 2)
    read()
    s.addEventListener('scroll', read, { passive: true })
    return () => s.removeEventListener('scroll', read)
  }, [rows, maxH])
  return (
    <div
      role="dialog"
      aria-label="Details"
      data-testid="lv-mon-det"
      data-sheet={sheet ? '' : undefined}
      data-k2-chamfer="surface"
      {...floatMaterial('popover', 'frost')}
      className="lv-mon-det absolute border border-[var(--color-border)] bg-[var(--color-bg)] [box-shadow:var(--shadow-3)] rounded-[var(--radius-box)]"
      style={{ maxHeight: Math.max(80, maxH) }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          onClose()
        }
      }}
    >
      <h4>
        Details
        <button
          ref={close}
          type="button"
          data-k2-focus=""
          aria-label="Close details"
          title="Close"
          onClick={onClose}
          className="lv-icon-btn inline-flex h-[24px] w-[24px] cursor-pointer items-center justify-center rounded-[var(--radius-field)] border-0 bg-transparent text-[var(--color-text-secondary)]"
        >
          <CloseGlyph size={14} />
        </button>
      </h4>
      <div ref={scroll} className="min-h-0 flex-initial overflow-y-auto" data-testid="lv-mon-det-scroll">
        <dl>
          {rows.map(([k, val]) => (
            <div key={k} className="contents">
              <dt>{k}</dt>
              <dd>{val}</dd>
            </div>
          ))}
        </dl>
      </div>
      {more ? (
        <div className="lv-mon-more" aria-hidden="true">
          <DownGlyph size={12} />
          More below
        </div>
      ) : null}
    </div>
  )
}
