// Home groups: the selected group's page in the Groups dialog (the right
// pane, or the whole dialog when it is one pane). At the top, fixed: its
// name (saved as you type; an empty one saves as the next "Group N", shown
// as the placeholder; a save that fails keeps the text, edges the field in
// the error colour and says so beside it, with Try again), "Most recent at the top" with its one-line
// explanation (the app's Toggle), and a segmented control, Agents | Icon
// (←/→ switch it):
//   - Agents: which agents are in the group, as checkboxes, saved live
//     (`GroupAgentsPane`, the same sources as Add to Home);
//   - Icon: the icon search, Recent and the grid in its own frame
//     (`GroupIconPicker`).
// Delete is pinned to the bottom: quiet, it asks once inline, and it is off
// on the last group with the reason in words.
//
// Presentational apart from the Agents segment, which reads the groups store;
// every other change goes out through a callback.

import { useEffect, useId, useRef, useState } from 'react'
import { CheckIcon, Trash2Icon } from 'lucide-react'
import { Button, Input, Toggle } from '@/components/ui'
import { K2_ICON_PX, K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { HOME_NAME_MAX } from '@/stores/homes'
import { Segmented } from '@/components/ui/Segmented'
import GroupIconPicker from './GroupIconPicker'
import { GROUP_DIALOG_FOOT_CLASS } from './group-bar-look'
import GroupAgentsPane from './GroupAgentsPane'
import type { GroupBarItem, GroupRenameResult } from './HomeGroupBar'
import { GROUP_NAME_LABEL, LAST_GROUP_TEXT } from './group-copy'

/** The right pane's two segments. */
export type GroupDetailTab = 'agents' | 'icon'
export const GROUP_DETAIL_TABS: ReadonlyArray<{ value: GroupDetailTab; label: string }> = [
  { value: 'agents', label: 'Agents' },
  { value: 'icon', label: 'Icon' },
]

export interface GroupDetailPaneProps {
  group: GroupBarItem
  /** The group's 0-based position (its dot's number). */
  index: number
  canDelete: boolean
  /** Save the name: `failed` (or a throw) shows the error at the field. */
  onRename(id: string, name: string): GroupRenameResult
  onSetIcon(id: string, icon: string | null): void
  onSetRecentFirst(id: string, on: boolean): void
  onDelete(id: string): void
  emptyNameFallback?(id: string): string
  /** Focus the name with its text selected (a group just added). */
  focusName?: boolean
  /** The segment shown (the dialog keeps it as the selection changes). */
  tab?: GroupDetailTab
  onTab?(tab: GroupDetailTab): void
}

const LABEL = 'text-[10px] font-semibold uppercase tracking-[0.06em] text-[var(--color-text-muted)]'
const SQUARE = { strokeWidth: K2_ICON_STROKE, strokeLinecap: 'square', strokeLinejoin: 'miter' } as const

/** What a failed rename says, beside the field. */
export const RENAME_FAILED_TEXT = 'Couldn’t save the name. Your text is kept here.'

export default function GroupDetailPane({
  group,
  index,
  canDelete,
  onRename,
  onSetIcon,
  onSetRecentFirst,
  onDelete,
  emptyNameFallback,
  focusName = false,
  tab = 'icon',
  onTab,
}: GroupDetailPaneProps): React.JSX.Element {
  const [name, setName] = useState(group.name)
  const [confirming, setConfirming] = useState(false)
  const [saved, setSaved] = useState(false)
  const [nameFailed, setNameFailed] = useState(false)
  const nameRef = useRef<HTMLInputElement>(null)
  const keepRef = useRef<HTMLButtonElement>(null)
  const delRef = useRef<HTMLButtonElement>(null)
  const uid = useId()
  const nameId = `${uid}-name`
  const recentId = `group-recent-${group.id}`
  useEffect(() => {
    if (!focusName) return
    nameRef.current?.focus()
    nameRef.current?.select()
  }, [focusName])
  useEffect(() => {
    if (confirming) keepRef.current?.focus()
  }, [confirming])

  const saveName = (text: string): void => {
    let ok: boolean
    try {
      ok = onRename(group.id, text) !== 'failed'
    } catch {
      ok = false
    }
    setNameFailed(!ok)
    setSaved(ok)
  }

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col" data-group-detail={group.id}>
      <div className="flex flex-shrink-0 flex-col px-5 pt-3.5">
        <div className="mb-1.5 flex items-center gap-1.5">
          <label htmlFor={nameId} className={LABEL}>
            {GROUP_NAME_LABEL}
          </label>
          {saved && (
            <span role="status" className="ml-auto inline-flex items-center gap-1 text-[10px] font-medium text-[var(--color-text-muted)]" data-group-saved="">
              <CheckIcon size={12} {...SQUARE} aria-hidden="true" />
              Saved
            </span>
          )}
        </div>
        <Input
          ref={nameRef}
          id={nameId}
          value={name}
          maxLength={HOME_NAME_MAX}
          placeholder={emptyNameFallback?.(group.id)}
          autoComplete="off"
          spellCheck={false}
          data-group-name-input=""
          aria-invalid={nameFailed || undefined}
          aria-describedby={nameFailed ? `${uid}-name-error` : undefined}
          className="h-7 w-full"
          onChange={(e) => {
            setName(e.target.value)
            saveName(e.target.value)
          }}
        />
        {nameFailed && (
          <div id={`${uid}-name-error`} role="alert" className="mt-1.5 text-[length:var(--type-xs)] leading-[1.45] text-[var(--color-status-error-text)]" data-group-name-error="">
            {RENAME_FAILED_TEXT}{' '}
            <button
              type="button"
              data-k2-focus=""
              data-group-name-retry=""
              className="no-drag inline-flex h-[22px] cursor-pointer items-center px-1.5 text-[var(--color-text-primary)] underline hover:bg-[var(--color-row-hover)] focus:outline-none [[data-style=glass]_&]:rounded-[var(--radius-pill)]"
              onClick={() => saveName(name)}
            >
              Try again
            </button>
          </div>
        )}
        <div className="mb-3 mt-2.5 flex items-center gap-4">
          <div className="min-w-0 flex-1">
            <span id={recentId} className="block text-[12px] font-medium text-[var(--color-text-primary)]">
              Most recent at the top
            </span>
            <span className="mt-0.5 block text-[10px] text-[var(--color-text-muted)]">The agent you last messaged moves up, below the pins.</span>
          </div>
          <Toggle
            size="sm"
            checked={group.recentFirst}
            aria-labelledby={recentId}
            data-group-recent-first={group.id}
            onChange={(on) => {
              onSetRecentFirst(group.id, on)
              if (!nameFailed) setSaved(true)
            }}
          />
        </div>
      </div>

      {/* The segments hug their choices (People and Channels join later),
          left-aligned, not the pane's width. */}
      <div className="flex flex-shrink-0 justify-start px-5 pb-2.5" data-group-detail-tabs="">
        <Segmented
          aria-label={`${group.name}: show`}
          value={tab}
          onChange={(t) => onTab?.(t)}
          options={GROUP_DETAIL_TABS.map((t) => ({ value: t.value, label: t.label, buttonProps: { 'data-group-detail-tab': t.value } }))}
        />
      </div>

      {tab === 'agents' ? (
        <GroupAgentsPane groupId={group.id} />
      ) : (
        <GroupIconPicker groupId={group.id} icon={group.icon} index={index} onSetIcon={onSetIcon} onPicked={() => !nameFailed && setSaved(true)} />
      )}

      <div className={`${GROUP_DIALOG_FOOT_CLASS} gap-3 px-5`} data-group-delete-row="" data-group-dialog-foot="detail">
        {confirming ? (
          <div className="flex min-w-0 flex-1 items-center gap-3" data-group-delete-confirm="">
            <span className="min-w-0 flex-1 text-[11px] leading-[1.45] text-[var(--color-text-secondary)]">
              Delete <b className="font-semibold text-[var(--color-text-primary)]">{group.name}</b>? Its agents stay where they are. Only the group goes.
            </span>
            <span className="flex flex-shrink-0 gap-2">
              <Button
                ref={keepRef}
                variant="quiet"
                className="h-7 !py-0"
                onClick={() => {
                  setConfirming(false)
                  requestAnimationFrame(() => delRef.current?.focus())
                }}
              >
                Keep it
              </Button>
              <Button
                variant="danger-muted"
                className="h-7 !py-0"
                data-group-delete-yes=""
                onClick={() => {
                  setConfirming(false)
                  onDelete(group.id)
                }}
              >
                Delete group
              </Button>
            </span>
          </div>
        ) : (
          <>
            <button
              ref={delRef}
              type="button"
              disabled={!canDelete}
              title={canDelete ? `Delete “${group.name}”` : LAST_GROUP_TEXT}
              aria-describedby={canDelete ? undefined : `${uid}-why`}
              data-group-delete=""
              data-k2-focus=""
              className="no-drag -ml-2 inline-flex h-7 cursor-pointer items-center gap-1.5 px-2 text-[11px] text-[var(--color-status-error-text)] hover:bg-[var(--color-row-hover)] focus:outline-none disabled:cursor-not-allowed disabled:text-[var(--color-text-muted)] disabled:hover:bg-transparent [[data-style=glass]_&]:rounded-[var(--radius-pill)]"
              onClick={() => setConfirming(true)}
            >
              <Trash2Icon size={K2_ICON_PX} {...SQUARE} aria-hidden="true" />
              Delete group
            </button>
            {!canDelete && (
              <span id={`${uid}-why`} className="text-[11px] text-[var(--color-text-muted)]">
                {LAST_GROUP_TEXT}
              </span>
            )}
          </>
        )}
      </div>
    </div>
  )
}
