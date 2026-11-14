// Home groups: the small card a group's icon opens (right-click, a long
// press on touch, or the "…" beside the current group's name). It holds the
// group's name (saved as you type), its icon (search, a Recent row, the
// grid, or no icon for the numbered dot) and Delete (confirm once; never
// the last group), its "Most recent at the top" switch, and "All groups…"
// (every group: name, icon, "Most recent at the top", order, Delete;
// `GroupSettingsList`). Escape or a click
// outside closes it.
//
// Presentational: everything it changes goes out through its callbacks, so
// K2 Design can restyle it without touching the logic (`useHomeGroups`).
// It floats on the app's popover layer (`useAnchoredMenu`), so no sidebar
// or scroller clips it.

import { useEffect, useMemo, useRef, useState } from 'react'
import { floatMaterial } from '@/components/ui/float-material'
import { Button } from '@/components/ui'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { HOME_NAME_MAX } from '@/stores/homes'
import { groupIcon, searchGroupIcons, type GroupIconDef } from '@/lib/group-icons'
import { readRecentGroupIcons, rememberGroupIcon } from '@/lib/group-icon-recents'
import { GroupDot, GroupIconSvg } from './GroupGlyph'
import GroupSettingsList from './GroupSettingsList'
import type { GroupBarItem } from './HomeGroupBar'

export interface GroupEditCardProps {
  group: GroupBarItem
  /** The group's 0-based position (its dot's number). */
  index: number
  /** False on the last group: Delete is off. */
  canDelete: boolean
  /** What the card sits against. */
  anchor: HTMLElement
  /** Opens toward the page: up from the bottom bar, down from the header. */
  prefer: 'up' | 'down'
  onRename(id: string, name: string): void
  onSetIcon(id: string, icon: string | null): void
  onDelete(id: string): void
  onClose(): void
  /** For the "Group settings" view: every group, in order. */
  groups: readonly GroupBarItem[]
  currentId: string
  onSetRecentFirst(id: string, on: boolean): void
  onReorderGroups(ids: string[]): void
  /** Show another group's card (from the settings list). */
  onEditGroup(id: string): void
  /** What an empty name saves as (the placeholder): the next "Group N". */
  emptyNameFallback?(id: string): string
}

const CELL =
  'flex h-7 w-7 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)] focus:outline-none focus-visible:bg-[var(--color-wash-1)] cursor-pointer'
const CELL_ON = '!text-[var(--color-accent)] bg-[var(--color-wash-1)]'

function IconCell({
  def,
  selected,
  onPick,
}: {
  def: GroupIconDef
  selected: boolean
  onPick(name: string): void
}): React.JSX.Element {
  return (
    <button
      type="button"
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

export default function GroupEditCard({
  group,
  index,
  canDelete,
  anchor,
  prefer,
  onRename,
  onSetIcon,
  onDelete,
  onClose,
  groups,
  currentId,
  onSetRecentFirst,
  onReorderGroups,
  onEditGroup,
  emptyNameFallback,
}: GroupEditCardProps): React.JSX.Element | null {
  const [view, setView] = useState<'group' | 'settings'>('group')
  const menu = useAnchoredMenu<HTMLElement>({
    open: true,
    onClose,
    width: 'min',
    minWidth: 264,
    estimatedHeight: 380,
    prefer,
    gap: 6,
  })
  // Placed against whichever element opened it (an icon, the "…").
  menu.anchorRef.current = anchor

  const [name, setName] = useState(group.name)
  const [query, setQuery] = useState('')
  const [confirming, setConfirming] = useState(false)
  const [recents, setRecents] = useState<string[]>(() => readRecentGroupIcons())
  const nameRef = useRef<HTMLInputElement>(null)

  // A new group's suggested name starts selected: typing replaces it.
  useEffect(() => {
    nameRef.current?.select()
  }, [group.id])

  const results = useMemo(() => searchGroupIcons(query), [query])
  const recentDefs = useMemo(
    () => recents.map((n) => groupIcon(n)).filter((d): d is GroupIconDef => d !== null),
    [recents],
  )

  const pick = (icon: string | null): void => {
    onSetIcon(group.id, icon)
    if (icon) setRecents(rememberGroupIcon(icon))
  }

  return menu.portal(
    <div
      ref={menu.menuRef}
      role="dialog"
      aria-label={`Edit group ${group.name}`}
      data-group-edit-card={group.id}
      data-placement={menu.placement}
      style={menu.style}
      {...floatMaterial('popover')}
      className="no-drag flex flex-col gap-2 border border-[var(--color-border)] bg-[var(--color-bg)] p-3 shadow-xl"
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          onClose()
        }
      }}
    >
      {view === 'settings' ? (
        <GroupSettingsList
          groups={groups}
          currentId={currentId}
          onRename={onRename}
          onSetRecentFirst={onSetRecentFirst}
          onReorderGroups={onReorderGroups}
          onDelete={onDelete}
          onEditGroup={(id) => (id === group.id ? setView('group') : onEditGroup(id))}
          onBack={() => setView('group')}
        />
      ) : (
      <>
      <label className="flex flex-col gap-1">
        <span className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">Name</span>
        <input
          ref={nameRef}
          autoFocus
          value={name}
          maxLength={HOME_NAME_MAX}
          data-group-name-input=""
          className="w-full border border-[var(--color-border)] bg-[var(--color-bg-surface)] px-2 py-1 text-xs text-[var(--color-text-primary)] focus:border-[var(--color-accent)] focus:outline-none"
          placeholder={emptyNameFallback?.(group.id)}
          onChange={(e) => {
            setName(e.target.value)
            // Saved as you type; an empty one saves as the placeholder's "Group N".
            onRename(group.id, e.target.value)
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter') onClose()
          }}
        />
      </label>

      <div className="flex flex-col gap-1">
        <span className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">Icon</span>
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
                <IconCell key={d.name} def={d} selected={group.icon === d.name} onPick={pick} />
              ))}
            </div>
          </div>
        )}
        <div className="grid max-h-44 grid-cols-8 gap-0.5 overflow-y-auto" data-group-icon-grid="">
          {!query.trim() && (
            <button
              type="button"
              className={`${CELL} ${group.icon === null ? CELL_ON : ''}`}
              aria-label="No icon (numbered dot)"
              aria-pressed={group.icon === null}
              title="No icon"
              data-group-icon-pick=""
              onClick={() => pick(null)}
            >
              <GroupDot n={index + 1} />
            </button>
          )}
          {results.map((d) => (
            <IconCell key={d.name} def={d} selected={group.icon === d.name} onPick={pick} />
          ))}
        </div>
        {results.length === 0 && (
          <p className="text-[11px] text-[var(--color-text-muted)]">No icons match “{query.trim()}”</p>
        )}
      </div>

      <label className="flex items-center justify-between gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer">
        Most recent at the top
        <input
          type="checkbox"
          role="switch"
          aria-checked={group.recentFirst}
          checked={group.recentFirst}
          data-group-card-recent-first=""
          className="cursor-pointer accent-[var(--color-accent)]"
          onChange={(e) => onSetRecentFirst(group.id, e.target.checked)}
        />
      </label>

      <button
        type="button"
        data-group-settings-open=""
        className="w-full px-2 py-1 text-left text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)] cursor-pointer"
        onClick={() => setView('settings')}
      >
        All groups…
      </button>

      <div className="border-t border-[var(--color-border)] pt-2">
        {confirming ? (
          <div className="flex items-center gap-2" data-group-delete-confirm="">
            <span className="min-w-0 flex-1 truncate text-[11px] text-[var(--color-text-secondary)]">
              Delete “{group.name}”? Its agents stay where they are.
            </span>
            <Button variant="ghost" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              data-group-delete-yes=""
              onClick={() => {
                onDelete(group.id)
                onClose()
              }}
            >
              Delete
            </Button>
          </div>
        ) : (
          <button
            type="button"
            className="w-full px-2 py-1 text-left text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)] disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent cursor-pointer"
            disabled={!canDelete}
            title={canDelete ? `Delete “${group.name}”` : 'The last group can’t be deleted'}
            data-group-delete=""
            onClick={() => setConfirming(true)}
          >
            Delete group
          </button>
        )}
      </div>
      </>
      )}
    </div>,
  )
}
