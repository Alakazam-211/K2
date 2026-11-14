// Home groups: the Groups dialog, centred on the window (K2 Design's mock).
// It opens from a right-click (or a long press, or the context-menu key) on
// a group's icon in the bar, with that group selected, and from the "…"
// beside the current group's name. K2's dialog shape (DialogScrim, then
// DialogFrame's Surface: elevation 5, the box radius, cut in Terminal),
// up to 880 wide, kept 16 px from the window's sides, and 80% of the
// window's height (`groupDialogSize`).
//
// Two panes: the group list (`GroupList`, 220 px: select, reorder by drag or
// ⌥↑/⌥↓, New group) and the selected group's settings (`GroupDetailPane`:
// name, "Most recent at the top", icon search and grid, Delete). Below
// 640 px of dialog width it is one pane that drills down: a right-click
// lands on that group's page (a back arrow returns to the list); "…" lands
// on the list. Done or Esc closes it; the bar puts focus back on what
// opened it.
//
// After a delete, "<Name> deleted." with Undo sits at the foot of the group
// list until the dialog closes (or the next delete); Undo puts the group
// back in its old place (`onRestore`). One pane, a delete returns to the
// list, where that line is.
//
// Presentational: it holds only which group is selected, which pane shows
// and the last delete; every change goes out through the callbacks
// (`useHomeGroups`).
// Portalled to <body>, so no frosted or transformed nav ancestor contains it.

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { ArrowLeftIcon } from 'lucide-react'
import { DialogFrame, DialogScrim } from '@/components/ui/Dialog'
import { Button } from '@/components/ui'
import { K2_ICON_PX, K2_ICON_STROKE } from '@/components/icons/K2Icons'
import GroupList, { type GroupListUndo } from './GroupList'
import GroupDetailPane, { type GroupDetailTab } from './GroupDetailPane'
import { GROUP_DIALOG_HEIGHT_SHARE, GROUP_DIALOG_LIST_PX, GROUP_DIALOG_MARGIN, GROUP_DIALOG_MAX, GROUP_DIALOG_TWO_PANE_MIN } from './group-bar-look'
import type { GroupBarItem, GroupRenameResult } from './HomeGroupBar'

export interface GroupEditModalProps {
  groups: readonly GroupBarItem[]
  currentId: string
  /** The group selected when it opens (the one right-clicked). */
  initialId: string
  /** One pane only: open on the list (the header's "…") or on the group's
   *  page (an icon). Default `group`. */
  startOn?: 'list' | 'group'
  /** The right pane's segment it opens on: Agents from "…", Icon from a
   *  right-click on a group's icon. Default `icon`. */
  startTab?: GroupDetailTab
  onRename(id: string, name: string): GroupRenameResult
  onSetIcon(id: string, icon: string | null): void
  onCreate(): string | null
  onDelete(id: string): void
  /** Undo the last delete (the group and its old index). Left out: no Undo. */
  onRestore?(group: GroupBarItem, index: number): boolean
  onSetRecentFirst(id: string, on: boolean): void
  onReorderGroups(ids: string[]): void
  emptyNameFallback?(id: string): string
  onClose(): void
}

/** The dialog's box: the max size, clamped to the window less the margins. */
export const GROUP_DIALOG_STYLE: React.CSSProperties = {
  width: `min(${GROUP_DIALOG_MAX.width}px, calc(var(--k2-vw) - ${2 * GROUP_DIALOG_MARGIN.x}px))`,
  height: `calc(var(--k2-vh) * ${GROUP_DIALOG_HEIGHT_SHARE})`,
  display: 'flex',
  flexDirection: 'column',
  overflow: 'hidden',
}

/** One pane below the breakpoint, measured on the dialog itself (so app
 *  zoom and the window's size both count). Unmeasured (no layout): two. */
function useOnePane(ref: React.RefObject<HTMLElement | null>): boolean {
  const [one, setOne] = useState(false)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const read = (): void => {
      const w = el.offsetWidth
      if (w > 0) setOne(w < GROUP_DIALOG_TWO_PANE_MIN)
    }
    read()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(read)
    ro.observe(el)
    return () => ro.disconnect()
  }, [ref])
  return one
}

export default function GroupEditModal({
  groups,
  currentId,
  initialId,
  startOn = 'group',
  startTab = 'icon',
  onRename,
  onSetIcon,
  onCreate,
  onDelete,
  onRestore,
  onSetRecentFirst,
  onReorderGroups,
  emptyNameFallback,
  onClose,
}: GroupEditModalProps): React.ReactPortal {
  const [selectedId, setSelectedId] = useState(initialId)
  const [justAdded, setJustAdded] = useState<string | null>(null)
  const [view, setView] = useState<'list' | 'group'>(startOn)
  const [tab, setTab] = useState<GroupDetailTab>(startTab)
  // The last delete, for Undo (until the dialog closes).
  const [undo, setUndo] = useState<{ group: GroupBarItem; index: number } | null>(null)
  const boxRef = useRef<HTMLDivElement>(null)
  const onePane = useOnePane(boxRef)
  // A deleted (or merged-away) selection falls back to the first group.
  useEffect(() => {
    if (!groups.some((g) => g.id === selectedId) && groups.length > 0) setSelectedId(groups[0].id)
  }, [groups, selectedId])

  const index = Math.max(0, groups.findIndex((g) => g.id === selectedId))
  const selected = groups[index]
  const showList = !onePane || view === 'list'
  const showGroup = !onePane || view === 'group'

  // Focus starts in the selected group's name (or, on the one-pane list,
  // its row), and moves there again when the pane changes.
  useEffect(() => {
    const box = boxRef.current
    if (!box || (showGroup && justAdded !== null && justAdded === selected?.id)) return
    const target = showGroup
      ? box.querySelector<HTMLInputElement>('[data-group-name-input]')
      : box.querySelector<HTMLElement>('[data-group-list-row][tabindex="0"]')
    if (!target) return
    target.focus({ preventScroll: true })
    if (target instanceof HTMLInputElement) target.setSelectionRange(target.value.length, target.value.length)
    // Only on open and when the pane changes (a new group focuses its own
    // name, selected, in GroupDetailPane).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onePane, view])

  const select = (id: string): void => {
    setSelectedId(id)
    if (onePane) setView('group')
  }

  // Focus goes to Undo once its line is drawn (the Delete button is gone).
  const focusUndo = useRef(false)
  useEffect(() => {
    if (!focusUndo.current || !undo) return
    focusUndo.current = false
    boxRef.current?.querySelector<HTMLElement>('[data-group-undo-button]')?.focus()
  }, [undo, view])

  const del = (id: string): void => {
    const at = groups.findIndex((g) => g.id === id)
    if (at < 0) return
    const group = groups[at]
    onDelete(id)
    if (!onRestore) return
    setUndo({ group, index: at })
    focusUndo.current = true
    if (onePane) setView('list')
  }

  const listUndo: GroupListUndo | undefined = undo
    ? {
        name: undo.group.name,
        onUndo: () => {
          if (!onRestore?.(undo.group, undo.index)) return
          setUndo(null)
          setSelectedId(undo.group.id)
          // Focus lands on the group, back in the list.
          requestAnimationFrame(() =>
            Array.from(boxRef.current?.querySelectorAll<HTMLElement>('[data-group-list-row]') ?? [])
              .find((el) => el.dataset.groupListRow === undo.group.id)
              ?.focus(),
          )
        },
      }
    : undefined

  return createPortal(
    <>
      <DialogScrim data-group-edit-scrim="" />
      <DialogFrame
        role="dialog"
        aria-modal="true"
        aria-label="Groups"
        data-group-edit-modal=""
        data-one-pane={onePane ? '' : undefined}
        data-view={onePane ? view : 'two'}
        onCancel={onClose}
        closeButton="inline"
        style={GROUP_DIALOG_STYLE}
        onKeyDown={(e) => {
          // The group list is a listbox, which the app leaves Esc to
          // (dialog-cancel): here Esc on it closes the dialog too.
          if (e.key === 'Escape' && !e.defaultPrevented && e.target instanceof Element && e.target.closest('[data-group-list-rows]')) {
            e.preventDefault()
            onClose()
          }
        }}
      >
        <div ref={boxRef} className="flex min-h-0 flex-1 flex-col">
          <div className="flex min-h-0 flex-1">
            {showList && (
              <div
                className={`flex min-h-0 flex-shrink-0 flex-col ${onePane ? 'w-full' : 'border-r border-[var(--color-border)]'}`}
                style={onePane ? undefined : { width: GROUP_DIALOG_LIST_PX }}
              >
                <GroupList
                  groups={groups}
                  currentId={currentId}
                  selectedId={selected?.id ?? ''}
                  drill={onePane}
                  onSelectGroup={select}
                  onReorderGroups={onReorderGroups}
                  undo={listUndo}
                  onCreate={() => {
                    const id = onCreate()
                    if (id) setJustAdded(id)
                    return id
                  }}
                />
              </div>
            )}
            {showGroup && selected && (
              <div className="flex min-h-0 min-w-0 flex-1 flex-col">
                <GroupDetailPane
                  key={selected.id}
                  group={selected}
                  index={index}
                  canDelete={groups.length > 1}
                  onRename={onRename}
                  onSetIcon={onSetIcon}
                  onSetRecentFirst={onSetRecentFirst}
                  onDelete={del}
                  emptyNameFallback={emptyNameFallback}
                  focusName={justAdded === selected.id}
                  tab={tab}
                  onTab={setTab}
                />
              </div>
            )}
          </div>
          <div className="order-first flex h-[52px] flex-shrink-0 items-center gap-2 border-b border-[var(--color-border)] pl-5 pr-3" data-group-edit-head="">
            {onePane && view === 'group' && selected ? (
              <>
                <button
                  type="button"
                  aria-label="Back to all groups"
                  title="All groups"
                  data-k2-focus=""
                  data-group-edit-back=""
                  className="-ml-3 flex h-[26px] w-[26px] flex-shrink-0 cursor-pointer items-center justify-center text-[var(--color-text-muted)] hover:bg-[var(--color-row-hover)] hover:text-[var(--color-text-primary)] focus:outline-none [[data-style=glass]_&]:rounded-[var(--radius-pill)]"
                  onClick={() => setView('list')}
                >
                  <ArrowLeftIcon size={K2_ICON_PX} strokeWidth={K2_ICON_STROKE} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" />
                </button>
                <h2 className="m-0 min-w-0 flex-1 truncate text-[15px] font-semibold text-[var(--color-text-primary)]">{selected.name}</h2>
              </>
            ) : (
              <h2 className="m-0 min-w-0 flex-1 truncate text-[15px] font-semibold text-[var(--color-text-primary)]">
                Groups
                <span className="ml-1.5 text-[11px] font-normal text-[var(--color-text-muted)]">{groups.length}</span>
              </h2>
            )}
            <Button variant="accent" className="h-7 flex-shrink-0 !px-4 !py-0 text-[12px]" data-group-edit-done="" onClick={onClose}>
              Done
            </Button>
          </div>
        </div>
      </DialogFrame>
    </>,
    document.body,
  )
}
