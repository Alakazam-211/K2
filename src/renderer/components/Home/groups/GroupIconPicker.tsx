// Home groups: the icon picker in the Groups dialog. The search field stays
// put above; under it, in its own scrolling frame (the grid scrolls, the
// name, switch, search and Delete never move): a Recent row (first "No
// icon", the numbered dot, then this computer's last picks,
// `group-icon-recents`), then every curated Lucide icon
// (`lib/group-icons.ts`). A search shows the matches instead. The grid is
// one tab stop: arrows move (up and down by the row you see), Home / End
// jump, Enter or Space picks. Presentational: a pick goes out through
// `onSetIcon`.

import { useMemo, useRef, useState } from 'react'
import { GROUP_ICONS, groupIcon, searchGroupIcons, type GroupIconDef } from '@/lib/group-icons'
import { readRecentGroupIcons, rememberGroupIcon } from '@/lib/group-icon-recents'
import { DIALOG_EDGE_TINT, ScrollEdgeFrame } from '@/components/ui/ScrollEdgeFade'
import { SearchField } from '@/components/ui/SearchField'
import { GroupDot, GroupIconSvg } from './GroupGlyph'

const CELL =
  'flex h-7 w-7 items-center justify-center rounded-[var(--radius-cell)] text-[var(--color-text-secondary)] hover:bg-[var(--color-row-hover)] hover:text-[var(--color-text-primary)] focus:outline-none cursor-pointer'
const CELLS = 'grid grid-cols-[repeat(auto-fill,28px)] justify-start gap-0.5'
const SUB = 'pb-1 pt-0.5 text-[10px] text-[var(--color-text-muted)]'

const label = (name: string): string => name.replace(/-\d+$/, '').replace(/-/g, ' ')

function IconCell({ def, selected, stop, onPick }: { def: GroupIconDef; selected: boolean; stop: boolean; onPick(name: string): void }): React.JSX.Element {
  return (
    <button
      type="button"
      role="gridcell"
      tabIndex={stop ? 0 : -1}
      data-k2-focus=""
      className={CELL}
      aria-label={label(def.name)}
      aria-pressed={selected}
      title={label(def.name)}
      data-group-icon-pick={def.name}
      onClick={() => onPick(def.name)}
    >
      <GroupIconSvg def={def} />
    </button>
  )
}

/** The cell an arrow key lands on, by what is on screen: left / right step
 *  through the order, up / down take the nearest cell in the row above or
 *  below. */
export function nextCell(cells: readonly HTMLElement[], from: number, key: string): number {
  if (key === 'ArrowLeft') return Math.max(0, from - 1)
  if (key === 'ArrowRight') return Math.min(cells.length - 1, from + 1)
  if (key === 'Home') return 0
  if (key === 'End') return cells.length - 1
  const at = cells[from]
  if (!at) return from
  const down = key === 'ArrowDown'
  const y = at.offsetTop
  const x = at.offsetLeft
  const rows = cells
    .map((c, i) => ({ i, y: c.offsetTop, x: c.offsetLeft }))
    .filter((c) => (down ? c.y > y : c.y < y))
  if (rows.length === 0) return from
  const rowY = down ? Math.min(...rows.map((c) => c.y)) : Math.max(...rows.map((c) => c.y))
  const row = rows.filter((c) => c.y === rowY)
  return row.reduce((best, c) => (Math.abs(c.x - x) < Math.abs(best.x - x) ? c : best), row[0]).i
}

export interface GroupIconPickerProps {
  groupId: string
  icon: string | null
  /** The group's 0-based position (the "No icon" dot's number). */
  index: number
  onSetIcon(id: string, icon: string | null): void
  /** After a pick lands (the pane says Saved). */
  onPicked?(): void
}

export default function GroupIconPicker({ groupId, icon, index, onSetIcon, onPicked }: GroupIconPickerProps): React.JSX.Element {
  const [query, setQuery] = useState('')
  const [recents, setRecents] = useState<string[]>(() => readRecentGroupIcons())
  const gridRef = useRef<HTMLDivElement>(null)
  const q = query.trim()
  const results = useMemo(() => (q ? searchGroupIcons(q) : GROUP_ICONS), [q])
  const recentDefs = useMemo(() => recents.map((n) => groupIcon(n)).filter((d): d is GroupIconDef => d !== null), [recents])

  const pick = (name: string | null): void => {
    onSetIcon(groupId, name)
    if (name) setRecents(rememberGroupIcon(name))
    onPicked?.()
  }

  // The roving stop: the picked icon where it shows (the first one in the
  // order), else "No icon".
  const order: (string | null)[] = q ? [null, ...results.map((d) => d.name)] : [null, ...recentDefs.map((d) => d.name), ...results.map((d) => d.name)]
  const stopAt = Math.max(0, order.indexOf(icon))
  let seen = -1
  const isStop = (): boolean => {
    seen += 1
    return seen === stopAt
  }

  const noIcon = (
    <button
      type="button"
      role="gridcell"
      tabIndex={isStop() ? 0 : -1}
      data-k2-focus=""
      className={CELL}
      aria-label="No icon (numbered dot)"
      aria-pressed={icon === null}
      title={`No icon: shows ${index + 1}`}
      data-group-icon-pick=""
      onClick={() => pick(null)}
    >
      <GroupDot n={index + 1} />
    </button>
  )

  return (
    <>
      <div className="flex-shrink-0 px-5 relative z-[2]" data-edge-header="">
        <SearchField
          className="w-full"
          value={query}
          placeholder="Search icons"
          aria-label="Search icons for this group"
          data-group-icon-search=""
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>
      <ScrollEdgeFrame className="min-h-0 flex-1" scrollClassName="overflow-y-auto overflow-x-hidden px-5 pb-2 pt-2.5" tint={DIALOG_EDGE_TINT} heavy testId="group-icon-frame">
        <div
          ref={gridRef}
          role="grid"
          aria-label="Icons"
          data-group-icon-picker={groupId}
          onKeyDown={(e) => {
            if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) return
            const cells = Array.from(gridRef.current?.querySelectorAll<HTMLElement>('[data-group-icon-pick]') ?? [])
            const from = cells.indexOf(e.target as HTMLElement)
            if (from < 0) return
            e.preventDefault()
            const to = nextCell(cells, from, e.key)
            cells[from].tabIndex = -1
            cells[to].tabIndex = 0
            cells[to].focus()
          }}
        >
          {q ? (
            <>
              <div className={SUB} role="status" data-group-icon-count="">
                {results.length} {results.length === 1 ? 'icon' : 'icons'} for “{q}”
              </div>
              <div className={CELLS} role="row" data-group-icon-grid="">
                {noIcon}
                {results.map((d) => (
                  <IconCell key={d.name} def={d} selected={icon === d.name} stop={isStop()} onPick={pick} />
                ))}
              </div>
              {results.length === 0 && (
                <p className="m-0 py-2 text-[11px] text-[var(--color-text-muted)]">No icons match. Try a plainer word, like “home” or “music”.</p>
              )}
            </>
          ) : (
            <>
              <div className={SUB}>Recent</div>
              <div className={`${CELLS} mb-2`} role="row">
                {noIcon}
                <span className="contents" data-group-icon-recent="">
                  {recentDefs.map((d) => (
                    <IconCell key={d.name} def={d} selected={icon === d.name} stop={isStop()} onPick={pick} />
                  ))}
                </span>
              </div>
              <div className={SUB}>All icons</div>
              <div className={CELLS} role="row" data-group-icon-grid="">
                {results.map((d) => (
                  <IconCell key={d.name} def={d} selected={icon === d.name} stop={isStop()} onPick={pick} />
                ))}
              </div>
            </>
          )}
        </div>
      </ScrollEdgeFrame>
    </>
  )
}
