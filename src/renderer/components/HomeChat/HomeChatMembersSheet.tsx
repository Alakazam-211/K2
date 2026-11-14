// A channel's members sheet (prd-home-people-channels-v1 §7.3, §8.2, §12,
// §13): its people, Add people, Remove (Owner and Admins, invite-only
// channels), Leave, Rename and Delete for those allowed, and "Agents with
// access" with an eye mark. Granting an agent access is a later slice (S8):
// this sheet lists what the daemon reports and offers no grant.
//
// The daemon checks every change against K2's ledger (HP-19); this sheet
// hides what the caller's role can't do (`lib/home-chat/permissions`).

import { useCallback, useEffect, useMemo, useState, type JSX } from 'react'
import { daemonErrorText } from '@/lib/daemon-error'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useHomesStore } from '@/stores/homes'
import type { HomeChatTarget } from '@/lib/home-chat/address'
import {
  addHomeChatPeople,
  deleteHomeChatChannel,
  fetchHomeChatMembers,
  leaveHomeChatChannel,
  removeHomeChatPerson,
  renameHomeChatChannel,
  type HomeChatMembers,
} from '@/lib/home-chat/api'
import { callerIsAdmin, channelPerms } from '@/lib/home-chat/permissions'
import { pickablePerson } from '@/lib/home-chat/people'
import { closeHomeChat, refreshHomeChatPeople, scopeForChatHost, useHomeChatStore } from '@/stores/home-chat'

const muted = 'text-[var(--color-text-muted)]'
const button =
  'k2-focus rounded-[var(--radius-field)] border border-[var(--color-border)] px-2 py-0.5 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-row-hover)] disabled:opacity-50'

/** Every Home row on `address` goes (leaving, deleting). */
export function removeChatRowsEverywhere(address: string): void {
  const st = useHomesStore.getState()
  for (const h of st.homes) if (h.rows.some((r) => r.address === address)) st.removeRow(h.id, address)
}

export function HomeChatMembersSheet({
  target,
  onClose,
  onChanged,
}: {
  target: Extract<HomeChatTarget, { kind: 'channel' }>
  onClose: () => void
  onChanged: () => void
}): JSX.Element {
  const hostKey = target.hostKey
  const scope = useMemo(() => scopeForChatHost(hostKey), [hostKey])
  const people = useHomeChatStore((s) => s.byHost[hostKey]?.people ?? null)
  const [members, setMembers] = useState<HomeChatMembers | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const [picked, setPicked] = useState<Set<string>>(new Set())
  const [renaming, setRenaming] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const reload = useCallback(() => {
    fetchHomeChatMembers(scope, { conv: target.conv }).then(setMembers, (e: unknown) => setError(daemonErrorText(e) || String(e)))
  }, [scope, target.conv])
  useEffect(() => {
    reload()
    void refreshHomeChatPeople(hostKey)
  }, [reload, hostKey])

  const meExt = people?.me.ext ?? ''
  const nameOf = (ext: string): string => {
    if (ext === meExt) return 'You'
    const p = people?.people.find((x) => x.ext === ext)
    return p ? p.displayName : ext === 'k2:owner' ? 'Owner' : 'Someone'
  }
  const inChannel = new Set((members?.people ?? []).map((p) => p.ext))
  const perms = channelPerms({
    callerExt: meExt,
    admin: callerIsAdmin(people),
    creator: members?.creator ?? '',
    member: inChannel.has(meExt),
    open: members?.open ?? false,
    archived: members?.state === 'archived',
  })
  const addable = (people?.people ?? []).filter((p) => pickablePerson(p) && !inChannel.has(p.ext))

  const run = async (fn: () => Promise<void>): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await fn()
      reload()
      onChanged()
    } catch (e) {
      setError(daemonErrorText(e) || String(e))
    } finally {
      setBusy(false)
    }
  }

  const leave = async (): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Leave ${members?.name ?? 'this channel'}?`,
      message: members?.open
        ? 'You can join it again from Add channel: it is open to everyone on this server.'
        : 'You lose access at once. Someone in the channel can add you back.',
      confirmLabel: 'Leave',
      destructive: true,
    })
    if (!ok) return
    await run(async () => {
      await leaveHomeChatChannel(scope, target.conv)
      removeChatRowsEverywhere(target.address)
      closeHomeChat()
    })
  }

  const archive = async (): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Delete ${members?.name ?? 'this channel'}?`,
      message: 'It is archived: everyone keeps its history, read-only, and no one can post in it.',
      confirmLabel: 'Delete',
      destructive: true,
    })
    if (!ok) return
    await run(() => deleteHomeChatChannel(scope, target.conv))
  }

  return (
    <div
      role="dialog"
      aria-label="Channel members"
      data-testid="home-chat-members"
      className="absolute right-3 top-12 z-40 flex max-h-[70%] w-72 flex-col border border-[var(--color-border)] bg-[var(--color-bg)] [box-shadow:var(--shadow-3)]"
      onKeyDown={(e) => {
        if (e.key === 'Escape') onClose()
      }}
    >
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-3 py-2">
        {renaming !== null ? (
          <form
            className="flex flex-1 gap-1"
            onSubmit={(e) => {
              e.preventDefault()
              const name = renaming.trim()
              if (!name) return
              void run(() => renameHomeChatChannel(scope, target.conv, name)).then(() => setRenaming(null))
            }}
          >
            <input
              autoFocus
              aria-label="Channel name"
              data-testid="home-chat-rename-input"
              value={renaming}
              maxLength={100}
              onChange={(e) => setRenaming(e.target.value)}
              className="min-w-0 flex-1 border border-[var(--color-border)] bg-transparent px-1.5 text-[12px]"
            />
            <button type="submit" className={button} disabled={busy}>
              Save
            </button>
          </form>
        ) : (
          <span className="flex-1 truncate text-[12px] font-medium text-[var(--color-text-primary)]">
            {members ? `${members.open ? '#' : '🔒'} ${members.name}` : 'Members'}
          </span>
        )}
        <button type="button" aria-label="Close" className={button} onClick={onClose}>
          ×
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-2 text-[12px]">
        {error && (
          <p className="mb-2 text-[11px] text-[var(--color-status-error-text)]" data-testid="home-chat-members-error">
            {error}
          </p>
        )}
        {members === null && !error && <p className={muted}>Loading…</p>}
        {members && (
          <>
            {members.state === 'archived' && <p className={`mb-2 text-[11px] ${muted}`}>Archived: read-only.</p>}
            <div className={`mb-1 text-[10px] uppercase tracking-wider ${muted}`}>People · {members.people.length}</div>
            <ul data-testid="home-chat-member-list">
              {members.people.map((p) => (
                <li key={p.ext} className="flex items-center gap-2 py-0.5" data-member={p.ext}>
                  <span className="flex-1 truncate">{nameOf(p.ext)}</span>
                  {p.state === 'pending_add' && <span className={`text-[10px] ${muted}`}>Adding…</span>}
                  {p.ext === members.creator && <span className={`text-[10px] ${muted}`}>Made it</span>}
                  {perms.removeOthers && p.ext !== meExt && (
                    <button
                      type="button"
                      className={button}
                      disabled={busy}
                      data-testid="home-chat-remove-member"
                      onClick={() => void run(() => removeHomeChatPerson(scope, target.conv, p.ext))}
                    >
                      Remove
                    </button>
                  )}
                </li>
              ))}
            </ul>
            {perms.addPeople && !adding && (
              <button type="button" className={`${button} mt-2`} data-testid="home-chat-add-people" onClick={() => setAdding(true)}>
                Add people
              </button>
            )}
            {adding && (
              <div className="mt-2" data-testid="home-chat-add-people-list">
                {addable.length === 0 ? (
                  <p className={muted}>Everyone you can add is already here.</p>
                ) : (
                  addable.map((p) => (
                    <label key={p.ext} className="flex items-center gap-2 py-0.5">
                      <input
                        type="checkbox"
                        checked={picked.has(p.ext)}
                        onChange={(e) =>
                          setPicked((cur) => {
                            const next = new Set(cur)
                            if (e.target.checked) next.add(p.ext)
                            else next.delete(p.ext)
                            return next
                          })
                        }
                      />
                      <span className="truncate">{p.displayName}</span>
                    </label>
                  ))
                )}
                <div className="mt-1 flex gap-1">
                  <button
                    type="button"
                    className={button}
                    disabled={busy || picked.size === 0}
                    onClick={() =>
                      void run(async () => {
                        await addHomeChatPeople(scope, target.conv, [...picked])
                        setPicked(new Set())
                        setAdding(false)
                      })
                    }
                  >
                    Add
                  </button>
                  <button type="button" className={button} onClick={() => setAdding(false)}>
                    Cancel
                  </button>
                </div>
              </div>
            )}
            <div className={`mb-1 mt-3 text-[10px] uppercase tracking-wider ${muted}`}>Agents with access</div>
            {members.agents.length === 0 ? (
              <p className={`text-[11px] ${muted}`} data-testid="home-chat-no-agents">
                No agent can read this channel.
              </p>
            ) : (
              <ul data-testid="home-chat-agent-list">
                {members.agents.map((a, i) => (
                  <li key={String(a.agent ?? a.name ?? i)} className="flex items-center gap-2 py-0.5">
                    <span aria-label="can read" title="Can read and post">
                      👁
                    </span>
                    <span className="truncate">{String(a.name ?? a.agent ?? 'Agent')}</span>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </div>
      {members && (
        <div className="flex flex-wrap gap-1 border-t border-[var(--color-border)] px-3 py-2">
          {perms.leave && (
            <button type="button" className={button} disabled={busy} data-testid="home-chat-leave" onClick={() => void leave()}>
              Leave
            </button>
          )}
          {perms.rename && renaming === null && (
            <button type="button" className={button} disabled={busy} data-testid="home-chat-rename" onClick={() => setRenaming(members.name)}>
              Rename
            </button>
          )}
          {perms.remove && (
            <button type="button" className={button} disabled={busy} data-testid="home-chat-delete" onClick={() => void archive()}>
              Delete
            </button>
          )}
        </div>
      )}
    </div>
  )
}
