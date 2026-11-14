// Home groups: the icon picker in the group editor. Search (name and plain
// words), a Recent row (this computer's last picks, `group-icon-recents`),
// "No icon" (the numbered dot), and the curated Lucide grid
// (`lib/group-icons.ts`). Presentational: a pick goes out through
// `onSetIcon`.

import { useMemo, useState } from 'react'
import { groupIcon, searchGroupIcons, type GroupIconDef } from '@/lib/group-icons'
import { readRecentGroupIcons, rememberGroupIcon } from '@/lib/group-icon-recents'
import { GroupDot, GroupIconSvg } from './GroupGlyph'

const CELL =
  'flex h-7 w-7 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)] focus:outline-none focus-visible:bg-[var(--color-wash-1)] cursor-pointer'
const CELL_ON = '!text-[var(--color-accent)] bg-[var(--color-wash-1)]'

function IconCell({ def, selected, onPick }: { def: GroupIconDef; selected: boolean; onPick(name: string): void }): React.JSX.Element {
  return (
    <button
      type="button"
      data-k2-focus=""
      className={`${CELL} ${selected ? CELL_ON : ''}`}
      aria-label={def.name.replace(/-/g, ' ')}
      aria-pressed={selected}
      title={def.name}
      data-group-icon-pick={def.name}
      onClick={() => onPick(def.name)}
    >
      <GroupIconSvg def={def} />
    </button>
  )
}

export interface GroupIconPickerProps {
  groupId: string
  icon: string | null
  /** The group's 0-based position (the "No icon" dot's number). */
  index: number
  onSetIcon(id: string, icon: string | null): void
}

export default function GroupIconPicker({ groupId, icon, index, onSetIcon }: GroupIconPickerProps): React.JSX.Element {
  const [query, setQuery] = useState('')
  const [recents, setRecents] = useState<string[]>(() => readRecentGroupIcons())
  const results = useMemo(() => searchGroupIcons(query), [query])
  const recentDefs = useMemo(() => recents.map((n) => groupIcon(n)).filter((d): d is GroupIconDef => d !== null), [recents])

  const pick = (name: string | null): void => {
    onSetIcon(groupId, name)
    if (name) setRecents(rememberGroupIcon(name))
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-1" data-group-icon-picker={groupId}>
      <input
        value={query}
        placeholder="Search icons"
        aria-label="Search icons"
        data-group-icon-search=""
        className="w-full border border-[var(--color-border)] bg-[var(--color-bg-surface)] px-2 py-1 text-xs text-[var(--color-text-primary)] focus:border-[var(--color-accent)] focus:outline-none"
        onChange={(e) => setQuery(e.target.value)}
      />
      {!query.trim() && recentDefs.length > 0 && (
        <div className="flex flex-col gap-0.5" data-group-icon-recent="">
          <span className="text-[10px] text-[var(--color-text-muted)]">Recent</span>
          <div className="flex flex-wrap gap-0.5">
            {recentDefs.map((d) => (
              <IconCell key={d.name} def={d} selected={icon === d.name} onPick={pick} />
            ))}
          </div>
        </div>
      )}
      <div className="grid min-h-0 flex-1 grid-cols-[repeat(auto-fill,minmax(28px,1fr))] content-start gap-0.5 overflow-y-auto" data-group-icon-grid="">
        {!query.trim() && (
          <button
            type="button"
            data-k2-focus=""
            className={`${CELL} ${icon === null ? CELL_ON : ''}`}
            aria-label="No icon (numbered dot)"
            aria-pressed={icon === null}
            title="No icon"
            data-group-icon-pick=""
            onClick={() => pick(null)}
          >
            <GroupDot n={index + 1} />
          </button>
        )}
        {results.map((d) => (
          <IconCell key={d.name} def={d} selected={icon === d.name} onPick={pick} />
        ))}
      </div>
      {results.length === 0 && <p className="text-[11px] text-[var(--color-text-muted)]">No icons match “{query.trim()}”</p>}
    </div>
  )
}
