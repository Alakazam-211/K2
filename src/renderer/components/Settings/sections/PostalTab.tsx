// Settings → Workspaces / Agents → <workspace> → Postal (Beta)
// (prd-postal-in-k2-v1 §5.5, vs-live P34). Postal lets an agent have
// friends: agents on other machines, at addresses like
// `alice::initech.postal.bot`, behind the server's Postal switch (off by
// default; Settings → General → Server). The daemon talks to the Postal
// network; its `plane.note` under the address says whether it is connected.
//
// Sections, all read from the daemon (postal-api.ts):
//   - Address: the workspace's Postal address, or (Owner/Admin) a form to
//     claim one;
//   - Friends: accepted friends, each with Remove and Block (each asks
//     first); a changed key says what it means and offers Trust new key;
//   - Invitations: incoming (Accept opens K2's code form when the request
//     carries a pairing code, marked on the row; Reject), outgoing (Waiting,
//     or Not sent yet before the network has it), the asker's note, and an
//     Invite form once the agent has an address;
//   - Blocked (only when there is one): Unblock lifts the block (the daemon's
//     remove), so the peer can send friend requests again;
//   - History;
//   - Routing: the default rule, in words.
//
// What happened after an action is said in the card where it happened. A
// refusal reads in plain words; a request that stopped waiting meanwhile
// reloads the lists and says so.
//
// The tab shows only while the server lists `postal` in `/boot-status`
// features, never for a Voice & Chat only login (usePostalTabShown).

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { SETTING_ROW_LABEL, SettingRow, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { Button, Input } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { ConfirmSheet } from './AccessDialogs'
import { PostalPairCodeForm } from './PostalPairCodeForm'
import { CardNotice, EmptyRow, InlineError, PeerRow, POSTAL_HINT, POSTAL_MEASURE, handleStatusText, type PostalNotice } from './PostalRows'
import {
  blockPostalFriend,
  claimPostalAddress,
  decidePostalRequest,
  fetchPostalFriends,
  fetchPostalStatus,
  fetchPostalViewerRole,
  invitePostalFriend,
  isPostalAddress,
  postalErrorCode,
  postalErrorText,
  postalPlainError,
  removePostalFriend,
  usePostalTabShown,
  type PostalFriend,
  type PostalFriendStatus,
  type PostalStatus,
} from './postal-api'

export const POSTAL_ROUTING_LINE =
  'Mail from a friend lands in this agent’s terminal and wakes it. Long messages, files, and mail that comes while it’s busy go to its inbox instead, with a note in its terminal.'
export const POSTAL_ROUTING_HINT = 'To change it for this agent or for one friend, the agent runs k2 postal route.'

export const POSTAL_MANIFEST: SettingEntry[] = [
  { id: 'projects.postal', section: 'projects', label: 'Postal address', description: "This agent's Postal address, so agents on other machines can be its friends", keywords: ['postal', 'postal.bot', 'address', 'handle', 'claim', 'friends', 'other machines', 'p5'], beta: true },
  { id: 'projects.postal-friends', section: 'projects', label: 'Postal friends', description: "Agents on other machines this agent can write to; remove or block one", keywords: ['postal', 'friends', 'remove', 'block', 'peer'], beta: true },
  { id: 'projects.postal-invitations', section: 'projects', label: 'Postal invitations', description: 'Friend requests in and out: accept, reject, or invite an agent on another machine', keywords: ['postal', 'invite', 'invitation', 'request', 'accept', 'reject', 'pending'], beta: true },
]

type Load<T> = { kind: 'loading' } | { kind: 'ready'; value: T } | { kind: 'error'; message: string }

const FRIEND_STATUSES: ReadonlySet<PostalFriendStatus> = new Set(['accepted', 'rekey_pending'])
const HISTORY_STATUSES: ReadonlySet<PostalFriendStatus> = new Set(['rejected', 'revoked'])

/** A Remove or Block waiting on the person's yes. */
type Confirm = { kind: 'remove' | 'block'; peer: string } | null

/** The confirm for Remove and Block: what each does, before it happens. */
const CONFIRM_WORDS: Record<'remove' | 'block', { title: (peer: string) => string; message: string; label: string }> = {
  block: {
    title: (peer) => `Block ${peer}?`,
    message: 'It ends the friendship and refuses its requests. You can unblock it later, but you’d need to be friends again.',
    label: 'Block',
  },
  remove: {
    title: (peer) => `Remove ${peer}?`,
    message: 'You’d need a new request to be friends again.',
    label: 'Remove',
  },
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
  // The request whose code form is open.
  const [codeFor, setCodeFor] = useState<string | null>(null)
  const [notice, setNotice] = useState<PostalNotice | null>(null)
  const [confirm, setConfirm] = useState<Confirm>(null)
  // Peers unblocked here: History says Unblocked, not Removed, for them.
  const [unblocked, setUnblocked] = useState<ReadonlySet<string>>(() => new Set())
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
    setConfirm(null)
    setUnblocked(new Set())
    setClaimAddress('')
    setInviteTo('')
    setInviteWhy('')
  }, [workspace])

  const reload = useCallback(() => setNonce((n) => n + 1), [])

  /** Run one write under `key`, from the card `at`; its error shows by that
   *  key, in plain words. `done` is the notice, or reads it from the write's
   *  result. A request that stopped waiting meanwhile reloads the lists and
   *  says so in the card. */
  const run = useCallback(
    async <T,>(key: string, at: PostalNotice['at'], write: () => Promise<T>, done?: string | ((result: T) => string), peer?: string): Promise<boolean> => {
      if (busy) return false
      setBusy(key)
      setNotice(null)
      setErrors((e) => {
        const rest = { ...e }
        delete rest[key]
        return rest
      })
      try {
        const result = await write()
        if (!mounted.current) return true
        if (done) setNotice({ text: typeof done === 'function' ? done(result) : done, at, tone: 'ok' })
        reload()
        return true
      } catch (err) {
        if (!mounted.current) return false
        if (postalErrorCode(err) === 'postal_not_pending') {
          setNotice({ text: `${postalPlainError(err, peer)} The list is up to date now.`, at, tone: 'error' })
          reload()
        } else {
          setErrors((e) => ({ ...e, [key]: `${postalPlainError(err)} Nothing changed; try again.` }))
        }
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
      blocked: list.filter((f) => f.status === 'blocked'),
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
      <SettingsLoadState kind="loading" title="Reading this agent’s Postal address…" testId="postal-status-loading" />
    )
  } else if (status.kind === 'error') {
    addressBody = (
      <SettingsLoadState
        kind="error"
        title="Couldn’t read Postal for this agent"
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
    const ok = await run('claim', 'address', () => claimPostalAddress(scope, workspace, address), `Claimed ${address}.`)
    if (ok && mounted.current) setClaimAddress('')
  }

  const submitInvite = async (): Promise<void> => {
    const to = inviteTo.trim()
    if (!isPostalAddress(to)) {
      setErrors((e) => ({ ...e, invite: 'Use a full address: handle::label.postal.bot' }))
      return
    }
    const ok = await run('invite', 'invitations', () => invitePostalFriend(scope, workspace, to, inviteWhy.trim()), (r) =>
      r?.sent === true ? `Invitation sent to ${to}.` : `Invitation to ${to} goes out once this server connects to the Postal network.`,
    )
    if (ok && mounted.current) {
      setInviteTo('')
      setInviteWhy('')
    }
  }

  /** Accept a request (or trust a changed key): K2's code form when it
   *  carries a pairing code, otherwise at once. */
  const accept = (f: PostalFriend, at: PostalNotice['at'], done: string): void => {
    if (f.codeWaiting) setCodeFor(f.id)
    else void run(`decide:${f.id}`, at, () => decidePostalRequest(scope, workspace, f.id, 'accept'), done, f.peer)
  }

  const codeForm = (f: PostalFriend, at: PostalNotice['at'], done: string): React.ReactNode =>
    canManage && f.codeWaiting === true && codeFor === f.id ? (
      <PostalPairCodeForm
        scope={scope}
        workspace={workspace}
        requestId={f.id}
        peer={f.peer}
        owner={f.ownerName ?? null}
        onDone={() => {
          setCodeFor(null)
          setNotice({ text: done, at, tone: 'ok' })
          reload()
        }}
        onCancel={() => setCodeFor(null)}
      />
    ) : undefined

  const loadState = (what: 'friends' | 'invitations'): React.ReactNode =>
    friends.kind === 'loading' ? (
      <SettingsLoadState kind="loading" title={`Reading ${what}…`} testId={`postal-${what}-loading`} />
    ) : friends.kind === 'error' ? (
      <SettingsLoadState
        kind="error"
        title={`Couldn’t read this agent’s ${what}`}
        detail={friends.message}
        action={<Button onClick={reload}>Try again</Button>}
        testId={`postal-${what}-error`}
      />
    ) : undefined

  const confirmWords = confirm ? CONFIRM_WORDS[confirm.kind] : null

  return (
    <div className="space-y-6" data-testid="postal-tab">
      <SettingsGroup
        title="Postal address"
        hint={<span className={`block ${POSTAL_MEASURE}`}>Postal lets this agent have friends: agents on other machines it can write to and hear from.</span>}
        card
        settingId="projects.postal"
        state={addressBody}
      >
        {status.kind === 'ready' && handle ? (
          <SettingRow
            label={<span className="font-mono break-all" data-testid="postal-address">{handle.address}</span>}
            hint={<span data-testid="postal-address-status">{handleStatusText(handle.status, handle.createdAt)}</span>}
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
            hint={<span className={`block ${POSTAL_MEASURE}`}>The full address, like alice::initech.postal.bot. Other agents use it to ask to be friends.</span>}
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
            <p className={`type-hint text-[var(--color-text-muted)] leading-relaxed ${POSTAL_MEASURE}`} data-testid="postal-plane-note">
              {status.value.plane.note}
            </p>
          </div>
        ) : null}
        <CardNotice notice={notice} at="address" />
      </SettingsGroup>

      <SettingsGroup title="Friends" card settingId="projects.postal-friends" state={loadState('friends')}>
        {groups.friends.length === 0 ? (
          <EmptyRow
            testId="postal-friends-empty"
            title="No friends yet"
            hint="Invite an agent on another machine below, or accept a request when one comes in."
          />
        ) : (
          groups.friends.map((f) => {
            const trusting = f.status === 'rekey_pending'
            const coding = trusting ? codeForm(f, 'friends', `Trusted ${f.peer}’s new key.`) : undefined
            return (
              <PeerRow
                key={f.id}
                friend={f}
                below={coding}
                error={errors[`remove:${f.peer}`] ?? errors[`block:${f.peer}`] ?? errors[`decide:${f.id}`] ?? null}
                actions={
                  canManage && !coding ? (
                    <>
                      {trusting ? (
                        <Button variant="quiet" disabled={busy !== null} onClick={() => accept(f, 'friends', `Trusted ${f.peer}’s new key.`)}>
                          Trust new key
                        </Button>
                      ) : null}
                      <Button variant="quiet" disabled={busy !== null} onClick={() => setConfirm({ kind: 'remove', peer: f.peer })}>
                        Remove
                      </Button>
                      <Button variant="danger-quiet" disabled={busy !== null} onClick={() => setConfirm({ kind: 'block', peer: f.peer })}>
                        Block
                      </Button>
                    </>
                  ) : undefined
                }
              />
            )
          })
        )}
        <CardNotice notice={notice} at="friends" />
      </SettingsGroup>

      <SettingsGroup
        title="Invitations"
        hint={
          <span className={`block ${POSTAL_MEASURE}`}>
            Requests never wake this agent. A request with a code check is accepted by typing the code shown on the other side’s screen.
          </span>
        }
        card
        settingId="projects.postal-invitations"
        state={loadState('invitations')}
      >
        {groups.incoming.length === 0 && groups.outgoing.length === 0 ? (
          <EmptyRow testId="postal-invitations-empty" title="No invitations waiting" />
        ) : null}
        {groups.incoming.map((f) => {
          // The open code form takes the row's full width under its lines;
          // the actions column hides meanwhile (Back brings it back).
          const coding = codeForm(f, 'invitations', `${f.peer} is now a friend.`)
          return (
            <PeerRow
              key={f.id}
              friend={f}
              error={errors[`decide:${f.id}`] ?? null}
              below={coding}
              actions={
                canManage && !coding ? (
                  <>
                    <Button variant="accent" disabled={busy !== null} onClick={() => accept(f, 'invitations', `${f.peer} is now a friend.`)}>
                      Accept
                    </Button>
                    <Button
                      variant="quiet"
                      disabled={busy !== null}
                      onClick={() =>
                        void run(`decide:${f.id}`, 'invitations', () => decidePostalRequest(scope, workspace, f.id, 'reject'), `Rejected ${f.peer}.`, f.peer)
                      }
                    >
                      Reject
                    </Button>
                  </>
                ) : undefined
              }
            />
          )
        })}
        {groups.outgoing.map((f) => (
          <PeerRow key={f.id} friend={f} />
        ))}
        {canManage && !handle ? (
          <EmptyRow testId="postal-invite-needs-address" title="Invite a friend" hint="Claim a Postal address for this agent first." />
        ) : null}
        {canManage && handle ? (
          <SettingRow
            stack
            label="Invite a friend"
            hint={<span className={`block ${POSTAL_MEASURE}`}>The other agent’s full address, and a note for its person if you like.</span>}
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
              />
              <Input
                className="min-w-48 flex-1"
                value={inviteWhy}
                onChange={(e) => setInviteWhy(e.target.value)}
                placeholder="Note (optional)"
                aria-label="Note with the invitation"
                maxLength={500}
              />
              <Button type="submit" variant="accent" disabled={busy !== null || inviteTo.trim() === ''}>
                {busy === 'invite' ? 'Inviting…' : 'Invite'}
              </Button>
            </form>
          </SettingRow>
        ) : null}
        <CardNotice notice={notice} at="invitations" />
      </SettingsGroup>

      {friends.kind === 'ready' && groups.blocked.length > 0 ? (
        <SettingsGroup title="Blocked" card testId="postal-blocked">
          {groups.blocked.map((f) => (
            <PeerRow
              key={f.id}
              friend={f}
              error={errors[`remove:${f.peer}`] ?? null}
              actions={
                canManage ? (
                  <Button
                    variant="quiet"
                    disabled={busy !== null}
                    onClick={() =>
                      void run(
                        `remove:${f.peer}`,
                        'friends',
                        () => removePostalFriend(scope, workspace, f.peer),
                        `Unblocked ${f.peer}. It can send friend requests again.`,
                      ).then((ok) => {
                        if (ok && mounted.current) setUnblocked((u) => new Set(u).add(f.peer))
                      })
                    }
                  >
                    Unblock
                  </Button>
                ) : undefined
              }
            />
          ))}
        </SettingsGroup>
      ) : null}

      {friends.kind === 'ready' && groups.history.length > 0 ? (
        <SettingsGroup title="History" card testId="postal-history">
          {groups.history.map((f) => (
            <PeerRow key={f.id} friend={f} unblocked={unblocked.has(f.peer)} />
          ))}
        </SettingsGroup>
      ) : null}

      <SettingsGroup title="Routing" card settingId="projects.postal-routing">
        <div className="px-4 py-3">
          <div className={SETTING_ROW_LABEL}>Where a friend’s mail goes</div>
          <p className={`${POSTAL_HINT}`} data-testid="postal-routing">
            {POSTAL_ROUTING_LINE}
          </p>
          <p className={`type-hint text-[var(--color-text-muted)] mt-1 leading-relaxed ${POSTAL_MEASURE}`} data-testid="postal-routing-hint">
            {POSTAL_ROUTING_HINT}
          </p>
        </div>
      </SettingsGroup>

      {confirm && confirmWords ? (
        <ConfirmSheet
          testId={`postal-${confirm.kind}-confirm`}
          title={confirmWords.title(confirm.peer)}
          message={confirmWords.message}
          confirmLabel={confirmWords.label}
          destructive
          onCancel={() => setConfirm(null)}
          onConfirm={() => {
            const { kind, peer } = confirm
            setConfirm(null)
            if (kind === 'block') void run(`block:${peer}`, 'friends', () => blockPostalFriend(scope, workspace, peer), `Blocked ${peer}. It’s under Blocked below.`)
            else void run(`remove:${peer}`, 'friends', () => removePostalFriend(scope, workspace, peer), `Removed ${peer}.`)
          }}
        />
      ) : null}
    </div>
  )
}
