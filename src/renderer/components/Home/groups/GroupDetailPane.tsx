// Home groups: the selected group's page in the Groups dialog (the right
// pane, or the whole dialog when it is one pane). At the top, fixed: its
// name (saved as you type; an empty one saves as the next "Group N", shown
// as the placeholder), "Most recent at the top" with its one-line
// explanation (the app's Toggle), and the icon search. Under them the icon
// grid scrolls in its own frame (`GroupIconPicker`). Delete is pinned to
// the bottom: quiet, it asks once inline, and it is off on the last group
// with the reason in words.
//
// Presentational: every change goes out through a callback.

import { useEffect, useId, useRef, useState } from 'react'
import { CheckIcon, Trash2Icon } from 'lucide-react'
import { Button, Input, Toggle } from '@/components/ui'
import { K2_ICON_PX, K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { HOME_NAME_MAX } from '@/stores/homes'
import GroupIconPicker from './GroupIconPicker'
import type { GroupBarItem } from './HomeGroupBar'

export interface GroupDetailPaneProps {
  group: GroupBarItem
  /** The group's 0-based position (its dot's number). */
  index: number
  canDelete: boolean
  onRename(id: string, name: string): void
  onSetIcon(id: string, icon: string | null): void
  onSetRecentFirst(id: string, on: boolean): void
  onDelete(id: string): void
  emptyNameFallback?(id: string): string
  /** Focus the name with its text selected (a group just added). */
  focusName?: boolean
}

const LABEL = 'text-[10px] font-semibold uppercase tracking-[0.06em] text-[var(--color-text-muted)]'
const SQUARE = { strokeWidth: K2_ICON_STROKE, strokeLinecap: 'square', strokeLinejoin: 'miter' } as const

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
}: GroupDetailPaneProps): React.JSX.Element {
  const [name, setName] = useState(group.name)
  const [confirming, setConfirming] = useState(false)
  const [saved, setSaved] = useState(false)
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

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col" data-group-detail={group.id}>
      <div className="flex flex-shrink-0 flex-col px-5 pt-3.5">
        <div className="mb-1.5 flex items-center gap-1.5">
          <label htmlFor={nameId} className={LABEL}>
            Name
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
          className="h-7 w-full"
          onChange={(e) => {
            setName(e.target.value)
            onRename(group.id, e.target.value)
            setSaved(true)
          }}
        />
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
              setSaved(true)
            }}
          />
        </div>
      </div>

      <GroupIconPicker groupId={group.id} icon={group.icon} index={index} onSetIcon={onSetIcon} onPicked={() => setSaved(true)} />

      <div className="flex min-h-12 flex-shrink-0 items-center gap-3 border-t border-[var(--color-border)] px-5 py-2" data-group-delete-row="">
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
              title={canDelete ? `Delete “${group.name}”` : 'The last group can’t be deleted'}
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
                The last group can’t be deleted.
              </span>
            )}
          </>
        )}
      </div>
    </div>
  )
}
