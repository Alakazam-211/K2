// A channel's members sheet (prd-home-people-channels-v1 §7.3, §8.2, §12,
// §13): its people, Invite (add people), Remove (Owner and Admins,
// invite-only channels), Leave, Rename and Delete for those allowed, and
// "Agents with access" with an eye mark. Granting an agent access is a later
// slice (S8): this sheet lists what the daemon reports and offers no grant.
//
// Look (K2 Design): K2's dialog shape (DialogScrim and DialogFrame: the
// dialog surface, the box radius in Glass, the cut in Terminal), portalled
// over the window. Head: the channel's glyph and name, and how many are in
// it. Each person is an avatar row (their colour and initials, the agent
// avatar's shape), their name, their role when the server says one (Owner
// and Admin callers see roles, §6.1), "Created it" for the channel's maker.
// Invite opens a checklist of who can be added, then Invite. The foot holds
// Leave, Rename and Delete. Esc, a press outside or × closes it.
//
// The daemon checks every change against K2's ledger (HP-19); this sheet
// hides what the caller's role can't do (`lib/home-chat/permissions`).

import { useCallback, useEffect, useId, useMemo, useState, type JSX } from 'react'
import { createPortal } from 'react-dom'
import { EyeIcon, UserPlusIcon } from 'lucide-react'
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
  type HomeChatPerson,
} from '@/lib/home-chat/api'
import { callerIsAdmin, channelPerms } from '@/lib/home-chat/permissions'
import { pickablePerson } from '@/lib/home-chat/people'
import { closeHomeChat, refreshHomeChatPeople, scopeForChatHost, useHomeChatStore } from '@/stores/home-chat'
import { DialogFrame, DialogScrim } from '@/components/ui/Dialog'
import { Button } from '@/components/ui/Button'
import { Input } from '@/components/ui/Input'
import { Checkbox } from '@/components/ui/Checkbox'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { HomeChatChannelGlyph, HomeChatPersonAvatar } from './HomeChatGlyphs'

const muted = 'text-[var(--color-text-muted)]'
const SECTION_HEAD = 'm-0 mb-1 px-2 type-hint font-semibold text-[var(--color-text-secondary)]'
const ROW = 'flex min-h-[44px] items-center gap-2.5 px-2 py-1.5'

/** Every Home row on `address` goes (leaving, deleting). */
export function removeChatRowsEverywhere(address: string): void {
  const st = useHomesStore.getState()
  for (const h of st.homes) if (h.rows.some((r) => r.address === address)) st.removeRow(h.id, address)
}

/** A role as the sheet says it: Owner, Admin; a member says nothing. */
export function memberRoleLabel(role: string | null | undefined): string | null {
  if (role === 'owner') return 'Owner'
  if (role === 'admin') return 'Admin'
  return null
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
  const titleId = useId()
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
  const personOf = (ext: string): HomeChatPerson | undefined => people?.people.find((x) => x.ext === ext)
  const nameOf = (ext: string): string => {
    if (ext === meExt) return 'You'
    const p = personOf(ext)
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

  const usernameOf = (ext: string): string => (ext === 'k2:owner' ? 'owner' : (personOf(ext)?.username ?? ext))

  return createPortal(
    <>
      <DialogScrim data-home-chat-members-scrim="" />
      <DialogFrame
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-testid="home-chat-members"
        onCancel={onClose}
        className="flex max-h-[min(560px,calc(var(--k2-vh)-48px))] w-[min(420px,calc(var(--k2-vw)-32px))] flex-col overflow-hidden"
      >
        <div className="flex-shrink-0 pt-4 pr-12 pb-3 pl-5" data-home-chat-members-head="">
          {renaming !== null ? (
            <form
              className="flex items-center gap-2"
              onSubmit={(e) => {
                e.preventDefault()
                const name = renaming.trim()
                if (!name) return
                void run(() => renameHomeChatChannel(scope, target.conv, name)).then(() => setRenaming(null))
              }}
            >
              <Input
                autoFocus
                aria-label="Channel name"
                data-testid="home-chat-rename-input"
                value={renaming}
                maxLength={100}
                onChange={(e) => setRenaming(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Escape') {
                    e.preventDefault()
                    e.stopPropagation()
                    setRenaming(null)
                  }
                }}
                className="min-w-0 flex-1"
              />
              <Button type="submit" variant="accent" disabled={busy || !renaming.trim()}>
                Save
              </Button>
              <Button variant="quiet" onClick={() => setRenaming(null)}>
                Cancel
              </Button>
            </form>
          ) : (
            <h2 id={titleId} className="m-0 flex min-w-0 items-center gap-1.5 text-[length:var(--type-xl)] font-semibold text-[var(--color-text-primary)]">
              {members && (
                <span className="inline-flex text-[var(--color-text-secondary)]">
                  <HomeChatChannelGlyph inviteOnly={!members.open} size={16} />
                </span>
              )}
              <span className="min-w-0 truncate">{members ? members.name : 'Members'}</span>
            </h2>
          )}
          {members && (
            <p className="m-0 mt-1 type-hint text-[var(--color-text-secondary)]" data-home-chat-members-sub="">
              {members.open ? 'Open to everyone on this server' : 'Invite only'} · {members.people.length === 1 ? '1 person' : `${members.people.length} people`}
              {members.state === 'archived' ? ' · Archived: read-only' : ''}
            </p>
          )}
        </div>

        {/* As tall as its people (the dialog grows to its 560px cap, then this scrolls). */}
        <div className="min-h-0 flex-shrink overflow-y-auto overflow-x-hidden px-3 pb-3" data-testid="home-chat-members-body">
          {error && (
            <p className="m-0 mb-2 px-2 type-hint text-[var(--color-status-error-text)]" data-testid="home-chat-members-error">
              {error}
            </p>
          )}
          {members === null && !error && <p className={`m-0 px-2 py-4 type-hint ${muted}`}>Loading…</p>}
          {members && (
            <>
              <h3 className={SECTION_HEAD}>People</h3>
              <ul className="m-0 list-none p-0" data-testid="home-chat-member-list">
                {members.people.map((p) => {
                  const person = personOf(p.ext)
                  const role = memberRoleLabel(person?.role ?? (p.ext === 'k2:owner' ? 'owner' : null))
                  const notes = [role, p.ext === members.creator ? 'Created it' : null, p.state === 'pending_add' ? 'Adding…' : null].filter(Boolean)
                  return (
                    <li key={p.ext} className={ROW} data-member={p.ext}>
                      <HomeChatPersonAvatar username={usernameOf(p.ext)} name={person?.displayName ?? (p.ext === 'k2:owner' ? 'Owner' : nameOf(p.ext))} picture={person?.picture ?? null} size={28} />
                      <span className="flex min-w-0 flex-1 flex-col">
                        <span className="truncate type-name text-[var(--color-text-primary)]">{nameOf(p.ext)}</span>
                        {notes.length > 0 && (
                          <span className="truncate type-hint text-[var(--color-text-secondary)]" data-member-role="">
                            {notes.join(' · ')}
                          </span>
                        )}
                      </span>
                      {perms.removeOthers && p.ext !== meExt && (
                        <Button variant="danger-quiet" disabled={busy} data-testid="home-chat-remove-member" onClick={() => void run(() => removeHomeChatPerson(scope, target.conv, p.ext))}>
                          Remove
                        </Button>
                      )}
                    </li>
                  )
                })}
              </ul>

              {perms.addPeople && adding && (
                <div className="mt-2" data-testid="home-chat-add-people-list">
                  <h3 className={SECTION_HEAD}>Invite people</h3>
                  {addable.length === 0 ? (
                    <p className={`m-0 px-2 py-1 type-hint ${muted}`}>Everyone you can add is already here.</p>
                  ) : (
                    addable.map((p) => (
                      <label key={p.ext} className={`${ROW} cursor-pointer hover:bg-[var(--color-row-hover)]`}>
                        <Checkbox
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
                        <HomeChatPersonAvatar username={p.username} name={p.displayName} picture={p.picture} size={24} />
                        <span className="min-w-0 flex-1 truncate type-name text-[var(--color-text-primary)]">{p.displayName}</span>
                      </label>
                    ))
                  )}
                  <div className="mt-2 flex gap-2 px-2">
                    <Button
                      variant="accent"
                      data-testid="home-chat-invite-confirm"
                      disabled={busy || picked.size === 0}
                      onClick={() =>
                        void run(async () => {
                          await addHomeChatPeople(scope, target.conv, [...picked])
                          setPicked(new Set())
                          setAdding(false)
                        })
                      }
                    >
                      {picked.size > 1 ? `Invite ${picked.size} people` : 'Invite'}
                    </Button>
                    <Button variant="quiet" onClick={() => setAdding(false)}>
                      Cancel
                    </Button>
                  </div>
                </div>
              )}

              <h3 className={`${SECTION_HEAD} mt-4`}>Agents with access</h3>
              {members.agents.length === 0 ? (
                <p className={`m-0 px-2 type-hint ${muted}`} data-testid="home-chat-no-agents">
                  No agent can read this channel.
                </p>
              ) : (
                <ul className="m-0 list-none p-0" data-testid="home-chat-agent-list">
                  {members.agents.map((a, i) => (
                    <li key={String(a.agent ?? a.name ?? i)} className={ROW}>
                      <span className="inline-flex text-[var(--color-text-secondary)]" aria-label="Can read and post" title="Can read and post">
                        <EyeIcon size={15} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
                      </span>
                      <span className="truncate type-name text-[var(--color-text-primary)]">{String(a.name ?? a.agent ?? 'Agent')}</span>
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>

        {members && (
          <div className="flex flex-shrink-0 flex-wrap items-center gap-2 border-t border-[var(--color-border)] px-5 py-3" data-home-chat-members-foot="">
            {perms.addPeople && !adding && (
              <Button variant="accent" data-testid="home-chat-add-people" onClick={() => setAdding(true)} className="inline-flex items-center gap-1.5">
                <UserPlusIcon size={14} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
                Invite people
              </Button>
            )}
            <span className="flex-1" />
            {perms.rename && renaming === null && (
              <Button variant="quiet" disabled={busy} data-testid="home-chat-rename" onClick={() => setRenaming(members.name)}>
                Rename
              </Button>
            )}
            {perms.leave && (
              <Button variant="quiet" disabled={busy} data-testid="home-chat-leave" onClick={() => void leave()}>
                Leave
              </Button>
            )}
            {perms.remove && (
              <Button variant="danger-quiet" disabled={busy} data-testid="home-chat-delete" onClick={() => void archive()}>
                Delete
              </Button>
            )}
          </div>
        )}
      </DialogFrame>
    </>,
    document.body,
  )
}
