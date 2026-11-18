// Settings → Workspaces / Agents → <workspace> → Postal (Beta)
// (prd-postal-in-k2-v1 §5.5, vs-live P34). Postal lets an agent have
// friends: agents on other machines, at addresses like
// `scout::acme.postal.bot`. This release ships the local half only, behind
// the server's Postal switch (off by default; Settings → General → Server).
// Nothing here talks to the Postal network yet; the daemon's `plane.note`
// says so under the address.
//
// Sections, all read from the daemon (postal-api.ts):
//   - Address: the workspace's Postal address, or (Owner/Admin) a form to
//     claim one;
//   - Friends: accepted friends (a changed key and blocked ones marked), each
//     with Remove and Block;
//   - Invitations: incoming (Accept / Reject), outgoing (waiting), history,
//     and an Invite form;
//   - Routing: the default rule, read-only until native Postal.
//
// The tab shows only while the server lists `postal` in `/boot-status`
// features, never for a Voice & Chat only login (usePostalTabShown). Every
// refusal shows the daemon's hint inline, never a stack trace. The look is
// the current Settings card style; K2 Design does a pass before Beta.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SettingRow, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { BetaTag, Button, Input, Tag, type TagTone } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import {
  blockPostalFriend,
  claimPostalAddress,
  decidePostalRequest,
  fetchPostalFriends,
  fetchPostalStatus,
  fetchPostalViewerRole,
  invitePostalFriend,
  isPostalAddress,
  postalErrorText,
  removePostalFriend,
  usePostalTabShown,
  type PostalFriend,
  type PostalFriendStatus,
  type PostalStatus,
} from './postal-api'

export const POSTAL_ROUTING_LINE =
  "Postal mail lands in this agent's terminal and wakes it (the default). Per-friend routing comes with native Postal."

export const POSTAL_MANIFEST: SettingEntry[] = [
  { id: 'projects.postal', section: 'projects', label: 'Postal address', description: "This agent's Postal address, so agents on other machines can be its friends", keywords: ['postal', 'postal.bot', 'address', 'handle', 'claim', 'friends', 'other machines', 'p5'], beta: true },
  { id: 'projects.postal-friends', section: 'projects', label: 'Postal friends', description: "Agents on other machines this agent can write to; remove or block one", keywords: ['postal', 'friends', 'remove', 'block', 'peer'], beta: true },
  { id: 'projects.postal-invitations', section: 'projects', label: 'Postal invitations', description: 'Friend requests in and out: accept, reject, or invite an agent on another machine', keywords: ['postal', 'invite', 'invitation', 'request', 'accept', 'reject', 'pending'], beta: true },
]

type Load<T> = { kind: 'loading' } | { kind: 'ready'; value: T } | { kind: 'error'; message: string }

const FRIEND_STATUSES: ReadonlySet<PostalFriendStatus> = new Set(['accepted', 'rekey_pending', 'blocked'])
const HISTORY_STATUSES: ReadonlySet<PostalFriendStatus> = new Set(['rejected', 'revoked'])

function statusTag(status: PostalFriendStatus): { text: string; tone: TagTone } | null {
  switch (status) {
    case 'rekey_pending':
      return { text: 'Key changed', tone: 'warn' }
    case 'blocked':
      return { text: 'Blocked', tone: 'danger' }
    case 'pending_out':
      return { text: 'Waiting', tone: 'muted' }
    case 'pending_in':
      return { text: 'Asks to be friends', tone: 'accent' }
    case 'rejected':
      return { text: 'Rejected', tone: 'muted' }
    case 'revoked':
      return { text: 'Removed', tone: 'muted' }
    default:
      return null
  }
}

function when(iso: string | null | undefined): string {
  if (!iso) return ''
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
}

function InlineError({ message, testId }: { message: string | null; testId?: string }): React.JSX.Element | null {
  if (!message) return null
  return (
    <p role="alert" data-testid={testId} className="type-hint text-[var(--color-status-error-soft)] mt-1 leading-relaxed max-w-[75ch] break-words">
      {message}
    </p>
  )
}

/** One peer in a list: its address, its owner's name, a status tag, and the
 *  row's actions on the right. */
function PeerRow({
  friend,
  actions,
  error,
}: {
  friend: PostalFriend
  actions?: React.ReactNode
  error?: string | null
}): React.JSX.Element {
  const tag = statusTag(friend.status)
  const meta = [friend.ownerName ? `Owner: ${friend.ownerName}` : '', friend.route ? `Route: ${friend.route}` : '', when(friend.decidedAt ?? friend.updatedAt ?? friend.createdAt)]
    .filter(Boolean)
    .join(' · ')
  return (
    <div className="px-4 py-3" data-testid="postal-peer-row" data-peer={friend.peer} data-status={friend.status}>
      <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-2">
        <div className="min-w-0 grow basis-40">
          <div className={`${SETTING_ROW_LABEL} flex flex-wrap items-center gap-2 break-all`}>
            <span className="font-mono">{friend.peer}</span>
            {tag ? <Tag tone={tag.tone}>{tag.text}</Tag> : null}
          </div>
          {meta ? <div className={SETTING_ROW_HINT}>{meta}</div> : null}
        </div>
        {actions ? <div className="flex flex-wrap items-center gap-2 flex-shrink-0">{actions}</div> : null}
      </div>
      <InlineError message={error ?? null} testId="postal-row-error" />
    </div>
  )
}

function EmptyRow({ title, hint, testId }: { title: string; hint?: string; testId?: string }): React.JSX.Element {
  return (
    <div className="px-4 py-3" data-testid={testId}>
      <div className={SETTING_ROW_LABEL}>{title}</div>
      {hint ? <div className={SETTING_ROW_HINT}>{hint}</div> : null}
    </div>
  )
}

export function PostalTab({
  project,
  scope,
}: {
  project: { id: string; path: string; name?: string }
  scope: ServerScope
}): React.JSX.Element | null {
  const shown = usePostalTabShown(scope)
  const workspace = project.path
  const [status, setStatus] = useState<Load<PostalStatus>>({ kind: 'loading' })
  const [friends, setFriends] = useState<Load<PostalFriend[]>>({ kind: 'loading' })
  const [role, setRole] = useState<string | null>(null)
  const [nonce, setNonce] = useState(0)
  /** One action in flight per key (`claim`, `invite`, `<verb>:<peer|id>`). */
  const [busy, setBusy] = useState<string | null>(null)
  /** Inline errors by the same key. */
  const [errors, setErrors] = useState<Record<string, string>>({})
  const [claimAddress, setClaimAddress] = useState('')
  const [inviteTo, setInviteTo] = useState('')
  const [inviteWhy, setInviteWhy] = useState('')
  const [notice, setNotice] = useState<string | null>(null)
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  useEffect(() => {
    if (!shown) return
    let live = true
    void fetchPostalViewerRole(scope).then((r) => {
      if (live) setRole(r)
    })
    fetchPostalStatus(scope, workspace)
      .then((v) => {
        if (live) setStatus({ kind: 'ready', value: v })
      })
      .catch((err) => {
        if (live) setStatus({ kind: 'error', message: postalErrorText(err) })
      })
    fetchPostalFriends(scope, workspace)
      .then((v) => {
        if (live) setFriends({ kind: 'ready', value: Array.isArray(v.friends) ? v.friends : [] })
      })
      .catch((err) => {
        if (live) setFriends({ kind: 'error', message: postalErrorText(err) })
      })
    return () => {
      live = false
    }
  }, [shown, scope, workspace, nonce])

  // Another workspace: start clean.
  useEffect(() => {
    setStatus({ kind: 'loading' })
    setFriends({ kind: 'loading' })
    setErrors({})
    setNotice(null)
    setClaimAddress('')
    setInviteTo('')
    setInviteWhy('')
  }, [workspace])

  const reload = useCallback(() => setNonce((n) => n + 1), [])

  /** Run one write under `key`; its error shows by that key. */
  const run = useCallback(
    async (key: string, write: () => Promise<unknown>, done?: string): Promise<boolean> => {
      if (busy) return false
      setBusy(key)
      setNotice(null)
      setErrors((e) => {
        const rest = { ...e }
        delete rest[key]
        return rest
      })
      try {
        await write()
        if (!mounted.current) return true
        if (done) setNotice(done)
        reload()
        return true
      } catch (err) {
        if (mounted.current) setErrors((e) => ({ ...e, [key]: postalErrorText(err) }))
        return false
      } finally {
        if (mounted.current) setBusy(null)
      }
    },
    [busy, reload],
  )

  const groups = useMemo(() => {
    const list = friends.kind === 'ready' ? friends.value : []
    return {
      friends: list.filter((f) => FRIEND_STATUSES.has(f.status)),
      incoming: list.filter((f) => f.status === 'pending_in'),
      outgoing: list.filter((f) => f.status === 'pending_out'),
      history: list.filter((f) => HISTORY_STATUSES.has(f.status)),
    }
  }, [friends])

  if (!shown) return null

  const canManage = role === 'owner' || role === 'admin'
  const handle = status.kind === 'ready' ? status.value.handle : null

  // ── Address ──
  let addressBody: React.ReactNode
  if (status.kind === 'loading') {
    addressBody = (
      <SettingsLoadState kind="loading" title="Reading this agent's Postal address…" testId="postal-status-loading" />
    )
  } else if (status.kind === 'error') {
    addressBody = (
      <SettingsLoadState
        kind="error"
        title="Couldn't read Postal for this agent"
        detail={status.message}
        action={<Button onClick={reload}>Try again</Button>}
        testId="postal-status-error"
      />
    )
  }

  const submitClaim = async (): Promise<void> => {
    const address = claimAddress.trim()
    if (!isPostalAddress(address)) {
      setErrors((e) => ({ ...e, claim: 'Use a full address: handle::label.postal.bot' }))
      return
    }
    const ok = await run('claim', () => claimPostalAddress(scope, workspace, address), `Claimed ${address}.`)
    if (ok && mounted.current) setClaimAddress('')
  }

  const submitInvite = async (): Promise<void> => {
    const to = inviteTo.trim()
    if (!isPostalAddress(to)) {
      setErrors((e) => ({ ...e, invite: 'Use a full address: handle::label.postal.bot' }))
      return
    }
    const ok = await run('invite', () => invitePostalFriend(scope, workspace, to, inviteWhy.trim()), `Invitation to ${to} saved.`)
    if (ok && mounted.current) {
      setInviteTo('')
      setInviteWhy('')
    }
  }

  const friendsState =
    friends.kind === 'loading' ? (
      <SettingsLoadState kind="loading" title="Reading friends…" testId="postal-friends-loading" />
    ) : friends.kind === 'error' ? (
      <SettingsLoadState
        kind="error"
        title="Couldn't read this agent's friends"
        detail={friends.message}
        action={<Button onClick={reload}>Try again</Button>}
        testId="postal-friends-error"
      />
    ) : undefined

  return (
    <div className="space-y-6" data-testid="postal-tab">
      <SettingsGroup
        title="Postal address"
        badge={<BetaTag />}
        hint="Postal lets this agent have friends: agents on other machines it can write to and hear from."
        card
        settingId="projects.postal"
        state={addressBody}
      >
        {status.kind === 'ready' && handle ? (
          <SettingRow
            label={<span className="font-mono break-all" data-testid="postal-address">{handle.address}</span>}
            hint={[
              handle.status ? `Status: ${handle.status}` : '',
              handle.createdAt ? `claimed ${when(handle.createdAt)}` : '',
            ]
              .filter(Boolean)
              .join(', ')}
          >
            <span className="type-hint text-[var(--color-text-secondary)]" data-testid="postal-counts">
              {status.value.counts.accepted} {status.value.counts.accepted === 1 ? 'friend' : 'friends'}
              {' · '}
              {status.value.counts.pendingIn} waiting on you
              {' · '}
              {status.value.counts.pendingOut} sent
            </span>
          </SettingRow>
        ) : null}
        {status.kind === 'ready' && !handle && canManage ? (
          <SettingRow
            stack
            label="Claim a Postal address"
            hint="The full address, like scout::acme.postal.bot. Other agents use it to ask to be friends."
            settingId="projects.postal-claim"
            below={<InlineError message={errors.claim ?? null} testId="postal-claim-error" />}
          >
            <form
              className="flex flex-wrap items-center gap-2 w-full"
              data-testid="postal-claim-form"
              onSubmit={(e) => {
                e.preventDefault()
                void submitClaim()
              }}
            >
              <Input
                className="w-72 max-w-full font-mono"
                value={claimAddress}
                onChange={(e) => setClaimAddress(e.target.value)}
                placeholder="handle::label.postal.bot"
                aria-label="Postal address to claim"
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
              />
              <Button type="submit" variant="accent" disabled={busy !== null || claimAddress.trim() === ''}>
                {busy === 'claim' ? 'Claiming…' : 'Claim'}
              </Button>
            </form>
          </SettingRow>
        ) : null}
        {status.kind === 'ready' && !handle && !canManage ? (
          <EmptyRow
            testId="postal-no-address"
            title="No Postal address yet"
            hint="The owner or an admin of this server can claim one."
          />
        ) : null}
        {status.kind === 'ready' && status.value.plane?.note ? (
          <div className="px-4 py-3">
            <p className="type-hint text-[var(--color-text-muted)] leading-relaxed max-w-[75ch]" data-testid="postal-plane-note">
              {status.value.plane.note}
            </p>
          </div>
        ) : null}
      </SettingsGroup>

      {notice ? (
        <p role="status" className="type-hint text-[var(--color-status-ok-soft)]" data-testid="postal-notice">
          {notice}
        </p>
      ) : null}

      <SettingsGroup title="Friends" card settingId="projects.postal-friends" state={friendsState}>
        {groups.friends.length === 0 ? (
          <EmptyRow
            testId="postal-friends-empty"
            title="No friends yet"
            hint="Invite an agent on another machine below, or accept a request when one comes in."
          />
        ) : (
          groups.friends.map((f) => (
            <PeerRow
              key={f.id}
              friend={f}
              error={errors[`remove:${f.peer}`] ?? errors[`block:${f.peer}`] ?? null}
              actions={
                canManage ? (
                  <>
                    <Button
                      variant="quiet"
                      disabled={busy !== null}
                      onClick={() => void run(`remove:${f.peer}`, () => removePostalFriend(scope, workspace, f.peer), `Removed ${f.peer}.`)}
                    >
                      Remove
                    </Button>
                    {f.status !== 'blocked' ? (
                      <Button
                        variant="danger-quiet"
                        disabled={busy !== null}
                        onClick={() => void run(`block:${f.peer}`, () => blockPostalFriend(scope, workspace, f.peer), `Blocked ${f.peer}.`)}
                      >
                        Block
                      </Button>
                    ) : null}
                  </>
                ) : undefined
              }
            />
          ))
        )}
      </SettingsGroup>

      <SettingsGroup
        title="Invitations"
        hint="Requests never wake this agent. Nothing is sent to other machines yet: the Postal network connection comes in a later release."
        card
        settingId="projects.postal-invitations"
        state={friendsState}
      >
        {groups.incoming.length === 0 && groups.outgoing.length === 0 ? (
          <EmptyRow testId="postal-invitations-empty" title="No invitations waiting" />
        ) : null}
        {groups.incoming.map((f) => (
          <PeerRow
            key={f.id}
            friend={f}
            error={errors[`decide:${f.id}`] ?? null}
            actions={
              canManage ? (
                <>
                  {f.codeWaiting ? (
                    <span className={SETTING_ROW_HINT} data-testid={`postal-code-note-${f.id}`}>
                      Has a pairing code: it can be accepted once the Postal network connection comes (a later release).
                    </span>
                  ) : null}
                  <Button
                    variant="accent"
                    disabled={busy !== null || f.codeWaiting === true}
                    onClick={() => void run(`decide:${f.id}`, () => decidePostalRequest(scope, workspace, f.id, 'accept'), `${f.peer} is now a friend.`)}
                  >
                    Accept
                  </Button>
                  <Button
                    variant="quiet"
                    disabled={busy !== null}
                    onClick={() => void run(`decide:${f.id}`, () => decidePostalRequest(scope, workspace, f.id, 'reject'), `Rejected ${f.peer}.`)}
                  >
                    Reject
                  </Button>
                </>
              ) : undefined
            }
          />
        ))}
        {groups.outgoing.map((f) => (
          <PeerRow key={f.id} friend={f} />
        ))}
        {canManage ? (
          <SettingRow
            stack
            label="Invite a friend"
            hint={
              handle
                ? 'The other agent’s full address, and a note for its person if you like.'
                : 'Claim a Postal address for this agent first.'
            }
            settingId="projects.postal-invite"
            below={<InlineError message={errors.invite ?? null} testId="postal-invite-error" />}
          >
            <form
              className="flex flex-wrap items-center gap-2 w-full"
              data-testid="postal-invite-form"
              onSubmit={(e) => {
                e.preventDefault()
                void submitInvite()
              }}
            >
              <Input
                className="w-72 max-w-full font-mono"
                value={inviteTo}
                onChange={(e) => setInviteTo(e.target.value)}
                placeholder="handle::label.postal.bot"
                aria-label="Address to invite"
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                disabled={!handle}
              />
              <Input
                className="w-72 max-w-full"
                value={inviteWhy}
                onChange={(e) => setInviteWhy(e.target.value)}
                placeholder="Note (optional)"
                aria-label="Note with the invitation"
                maxLength={500}
                disabled={!handle}
              />
              <Button type="submit" variant="quiet" disabled={!handle || busy !== null || inviteTo.trim() === ''}>
                {busy === 'invite' ? 'Inviting…' : 'Invite'}
              </Button>
            </form>
          </SettingRow>
        ) : null}
      </SettingsGroup>

      {friends.kind === 'ready' && groups.history.length > 0 ? (
        <SettingsGroup title="History" card testId="postal-history">
          {groups.history.map((f) => (
            <PeerRow key={f.id} friend={f} />
          ))}
        </SettingsGroup>
      ) : null}

      <SettingsGroup title="Routing" card settingId="projects.postal-routing">
        <div className="px-4 py-3">
          <div className={SETTING_ROW_LABEL}>Default rule</div>
          <p className={SETTING_ROW_HINT} data-testid="postal-routing">
            {POSTAL_ROUTING_LINE}
          </p>
        </div>
      </SettingsGroup>
    </div>
  )
}
