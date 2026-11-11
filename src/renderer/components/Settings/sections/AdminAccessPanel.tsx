// Settings → Access → Admin Access: the people who can sign in to this
// server, with their role, and the password rules. One column at the
// Settings content width, like Tunnel and General:
//
//   People          one row per person: picture, name, username, what's
//                   special about them (You, Temporary password, Off), the
//                   role in the shared field column, and a "…" menu for the
//                   rest (name and picture, reset password, sign-in on/off,
//                   cloning, remove). "Add person" opens a dialog.
//   Password rules  what every password must have; each change saves. The
//                   length field sits in the same column as the roles.
//
// Every dialog here (add, reset, name, remove, make Owner) is the page's
// own sheet, one width, opening at one spot. A failed row change says what
// failed and what to do on the row, with the server's reason after it.
//
// Lock-out guard: a person can't demote, turn off or remove their own
// login, and the only Owner login can't be demoted by itself. The daemon
// checks roles too (remove, reset password and roles are Owner-only;
// an Admin adds people, turns sign-in on and off, and moves people between
// Member and Voice & Chat only).

import React, { useEffect, useMemo, useRef, useState } from 'react'
import {
  SettingRow,
  SettingsGroup,
  SettingDropdown,
  SettingsLoadState,
  SaveNote,
  SaveError,
  useSaveStatus,
  SETTING_FIELD_WIDTH,
  SETTING_ROW_HINT,
  SETTING_LIST_ROW_HOVER,
} from '../controls/SettingControls'
import { Button, Input, OverflowMenu, SearchField, Tag, Toggle, type OverflowMenuItem } from '@/components/ui'
import { PresenceChip } from '@/components/Presence/PresenceChip'
import { setPersonName } from '@/components/Presence/person-name-api'
import { realName } from '@/lib/person-name'
import { primaryScope } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'
import { useServerSupports, featureMinVersion } from '@/lib/server-capabilities'
import {
  canSetRole,
  needsEnableAsMember,
  parseK2Role,
  parseShares,
  roleOptions,
  VOICE_CHAT_LABEL,
  type K2Role,
  type K2User,
} from './connect-user-roles'
import { VoiceChatSharesEditor } from './VoiceChatSharesEditor'
import { WasViewerBadge, WasViewerNotice } from './WasViewerNotice'
import { MUST_CHANGE_NOTE } from './AppUserDialogs'
import { OwnerAccountTag, ownerLinkAccess, useOwnerAccounts } from './OwnerAccountToggle'
import { threadPushAvailable } from './useNotifyPrefs'
import { SewAdminAccessState } from './SewAccessParts'
import { AddPersonDialog, ConfirmSheet, NameAndPictureDialog, ResetPasswordDialog } from './AdminAccessDialogs'
import {
  DEFAULT_POLICY,
  errReply,
  errText,
  localServerAddress,
  parsePolicy,
  userGet,
  userPost,
  whoamiGet,
  type PasswordPolicy,
} from './admin-access-api'

const ROLE_RANK: Record<string, number> = { owner: 0, admin: 1, member: 2, voice_chat: 3 }

/** A role as a person reads it. */
function roleLabel(raw: string | undefined): string {
  if (raw === 'voice_chat') return VOICE_CHAT_LABEL
  if (!raw) return 'Member'
  return raw[0].toUpperCase() + raw.slice(1)
}

/** The roles `actor` may give (§5.3, Q1): the Owner every role; an Admin
 *  moves people between Member and the Voice & Chat only role, on a server
 *  that knows the latter. */
function pickerOptions(actor: K2Role | null, voiceChatRole: boolean): Array<{ value: K2Role; label: string }> {
  const all = roleOptions(voiceChatRole)
  if (actor === 'owner') return [...all]
  if (actor === 'admin') return all.filter((o) => o.value === 'member' || o.value === 'voice_chat')
  return []
}

interface Who {
  loaded: boolean
  role: K2Role | null
  /** The daemon's owner token (the desktop app on its own computer). */
  ownerToken: boolean
  /** The viewer's login, when they signed in with one. */
  username: string | null
  error: string | null
}

type Load = 'loading' | 'ok' | { error: string }
type Dialog =
  | { kind: 'add' }
  | { kind: 'reset'; user: K2User }
  | { kind: 'name'; user: K2User }
  | { kind: 'remove'; user: K2User }
  | { kind: 'owner'; user: K2User }
  | null

/** A row change that didn't land: what failed, what to do now, and the
 *  server's own reason (none when the page already says it in its words). */
interface RowFail {
  what: string
  next: string
  detail: string | null
}
type RowState = 'idle' | 'saved' | RowFail

/** One state per person row: "Saved" for a moment, or why it failed. */
function useRowStatus(): { of: (u: string) => RowState; saved: (u: string) => void; failed: (u: string, f: RowFail) => void } {
  const [map, setMap] = useState<Record<string, RowState>>({})
  const timers = useRef<Record<string, ReturnType<typeof setTimeout>>>({})
  useEffect(() => () => Object.values(timers.current).forEach(clearTimeout), [])
  const set = (u: string, s: RowState): void => {
    clearTimeout(timers.current[u])
    setMap((m) => ({ ...m, [u]: s }))
    if (s === 'saved') timers.current[u] = setTimeout(() => setMap((m) => ({ ...m, [u]: 'idle' })), 2000)
  }
  return { of: (u) => map[u] ?? 'idle', saved: (u) => set(u, 'saved'), failed: (u, f) => set(u, f) }
}

/** What each row change is, for its failure line. */
type RowChange = 'role' | 'sign-in on' | 'sign-in off' | 'clone' | 'remove'

/** A failed row change in the page's words. A refusal the page knows
 *  (an Admin asking for a role change only an Owner makes) is said
 *  plainly, with no server text; anything else keeps the server's reason. */
function rowFail(change: RowChange, username: string, code: string | null, detail: string): RowFail {
  if (change === 'role') {
    return code === 'role_change_not_allowed'
      ? { what: `Couldn’t change ${username}’s role.`, next: 'Only an Owner can make this change. Ask an Owner of this server.', detail: null }
      : { what: `Couldn’t change ${username}’s role.`, next: 'Their role is unchanged. Try again in a moment.', detail }
  }
  if (change === 'remove') return { what: `Couldn’t remove ${username}.`, next: 'They can still sign in. Try again in a moment.', detail }
  if (change === 'clone') return { what: `Couldn’t change whether ${username} can clone agents.`, next: 'Nothing changed. Try again in a moment.', detail }
  return { what: `Couldn’t turn ${change === 'sign-in on' ? 'on' : 'off'} sign-in for ${username}.`, next: 'Nothing changed. Try again in a moment.', detail }
}

/** A thrown error's words (a network failure, not a server answer). */
const thrown = (e: unknown): string => (e instanceof Error ? e.message : String(e))

export function AdminAccessPanel(): React.JSX.Element {
  const [who, setWho] = useState<Who>({ loaded: false, role: null, ownerToken: false, username: null, error: null })
  const [users, setUsers] = useState<K2User[]>([])
  const [usersLoad, setUsersLoad] = useState<Load>('loading')
  const [policy, setPolicy] = useState<PasswordPolicy>(DEFAULT_POLICY)
  const [policyLoad, setPolicyLoad] = useState<Load>('loading')
  const [minDraft, setMinDraft] = useState(String(DEFAULT_POLICY.minLength))
  const [dialog, setDialog] = useState<Dialog>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [enableBusy, setEnableBusy] = useState<string | null>(null)
  const [serverAddress, setServerAddress] = useState<string | null>(null)
  const rows = useRowStatus()
  const minSave = useSaveStatus()
  const ruleSave = { requireNumber: useSaveStatus(), requireUppercase: useSaveStatus(), requireSpecial: useSaveStatus() }

  const activeHost = useConnectHostStore((s) => s.activeHost)
  const isRemote = activeHost !== 'local'
  const serverVersion = useConnectHostStore((s) => s.serverVersion)
  const supportsRoles = useServerSupports('roles')
  const oldHostNoRoles = isRemote && serverVersion !== null && !supportsRoles
  const voiceChatRole = primaryScope().serverSupports('voice-chat-role-v1')

  const canManage = who.role === 'owner' || who.role === 'admin'
  const isOwner = who.role === 'owner'
  const ownerLink = threadPushAvailable() ? ownerLinkAccess(who.ownerToken, who.role) : 'hidden'
  const ownerAccounts = useOwnerAccounts(who.loaded && ownerLink !== 'hidden')

  const loadWho = async (): Promise<K2Role | null> => {
    try {
      const res = await whoamiGet()
      if (!res.ok) {
        // A refused token is "not allowed here", not a failure to explain.
        setWho({ loaded: true, role: null, ownerToken: false, username: null, error: res.status === 403 ? null : await errText(res) })
        return null
      }
      const data = (await res.json()) as { role?: string; owner?: boolean; username?: unknown }
      const role: K2Role | null = parseK2Role(data.role) ?? (data.owner ? 'owner' : null)
      setWho({
        loaded: true,
        role,
        ownerToken: data.owner === true,
        username: data.owner === true ? null : typeof data.username === 'string' ? data.username.toLowerCase() : null,
        error: null,
      })
      return role
    } catch (e) {
      setWho({ loaded: true, role: null, ownerToken: false, username: null, error: e instanceof Error ? e.message : String(e) })
      return null
    }
  }

  const loadUsers = async (): Promise<void> => {
    try {
      const res = await userGet('')
      if (!res.ok) {
        setUsersLoad({ error: await errText(res) })
        return
      }
      const data = (await res.json()) as { users?: K2User[] }
      setUsers(Array.isArray(data.users) ? data.users : [])
      setUsersLoad('ok')
    } catch (e) {
      setUsersLoad({ error: e instanceof Error ? e.message : String(e) })
    }
  }

  const loadPolicy = async (): Promise<void> => {
    try {
      const res = await userGet('/policy')
      if (!res.ok) {
        setPolicyLoad({ error: await errText(res) })
        return
      }
      const p = parsePolicy(await res.json())
      setPolicy(p)
      setMinDraft(String(p.minLength))
      setPolicyLoad('ok')
    } catch (e) {
      setPolicyLoad({ error: e instanceof Error ? e.message : String(e) })
    }
  }

  const loadAll = async (): Promise<void> => {
    setUsersLoad('loading')
    setPolicyLoad('loading')
    const role = await loadWho()
    if (role === 'owner' || role === 'admin') {
      void loadUsers()
      void loadPolicy()
    }
  }

  useEffect(() => {
    void loadAll()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isRemote])

  // The address a new person types: this computer's tunnel address, or
  // the saved address of the server this window is on.
  useEffect(() => {
    if (activeHost !== 'local') {
      const h = activeHost
      setServerAddress(`${h.secure ? 'https' : 'http'}://${h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`}`)
      return
    }
    let live = true
    void localServerAddress().then((a) => live && setServerAddress(a))
    return () => {
      live = false
    }
  }, [activeHost])

  // ── People actions ────────────────────────────────────────────────────
  const patch = (username: string, p: Partial<K2User>): void =>
    setUsers((prev) => prev.map((u) => (u.username === username ? { ...u, ...p } : u)))

  /** Optimistic write: apply, POST, keep on success (and re-read), put it
   *  back and say why on failure. */
  const write = async (
    change: RowChange,
    username: string,
    route: string,
    body: Record<string, unknown>,
    apply: Partial<K2User>,
    undo: Partial<K2User>,
  ): Promise<void> => {
    patch(username, apply)
    try {
      const res = await userPost(route, body)
      if (!res.ok) {
        patch(username, undo)
        const { code, text } = await errReply(res)
        rows.failed(username, rowFail(change, username, code, text))
        return
      }
      rows.saved(username)
      void loadUsers()
    } catch (e) {
      patch(username, undo)
      rows.failed(username, rowFail(change, username, null, thrown(e)))
    }
  }

  const setRole = (u: K2User, role: K2Role): Promise<void> =>
    write('role', u.username, '/set-role', { username: u.username, role }, { role }, { role: u.role })

  /** Making someone an Owner asks first; other roles save at once. */
  const changeRole = (u: K2User, role: K2Role): void => {
    if (role === u.role) return
    if (role === 'owner') setDialog({ kind: 'owner', user: u })
    else void setRole(u, role)
  }

  const setSignIn = (u: K2User, on: boolean): Promise<void> =>
    write(on ? 'sign-in on' : 'sign-in off', u.username, '/set-disabled', { username: u.username, disabled: !on }, { disabled: !on }, { disabled: u.disabled })

  const setClone = (u: K2User, on: boolean): Promise<void> =>
    write('clone', u.username, '/set-can-clone', { username: u.username, canClone: on }, { canClone: on }, { canClone: u.canClone })

  const remove = async (u: K2User): Promise<void> => {
    try {
      const res = await userPost('/remove', { username: u.username })
      if (!res.ok) {
        const { code, text } = await errReply(res)
        rows.failed(u.username, rowFail('remove', u.username, code, text))
        return
      }
      setNotice(`${u.username} was removed. They can’t sign in to this server any more.`)
      await loadUsers()
    } catch (e) {
      rows.failed(u.username, rowFail('remove', u.username, null, thrown(e)))
    }
  }

  const enableAsMember = async (username: string): Promise<void> => {
    setEnableBusy(username)
    try {
      const res = await userPost('/set-disabled', { username, disabled: false })
      if (!res.ok) {
        const { code, text } = await errReply(res)
        rows.failed(username, rowFail('sign-in on', username, code, text))
      } else {
        rows.saved(username)
        await loadUsers()
      }
    } catch (e) {
      rows.failed(username, rowFail('sign-in on', username, null, thrown(e)))
    } finally {
      setEnableBusy(null)
    }
  }

  const saveName = async (username: string, firstName: string, lastName: string): Promise<string | null> => {
    try {
      await setPersonName(primaryScope(), { person: username, firstName: firstName || null, lastName: lastName || null })
      rows.saved(username)
      await loadUsers()
      return null
    } catch (e) {
      return e instanceof Error ? e.message : String(e)
    }
  }

  const saveShares = async (username: string, agents: string[], gardens: string[]): Promise<string | null> => {
    try {
      const res = await userPost('/set-shares', { username, agents, gardens })
      if (!res.ok) return await errText(res)
      await loadUsers()
      return null
    } catch (e) {
      return e instanceof Error ? e.message : String(e)
    }
  }

  // ── Password rules ────────────────────────────────────────────────────
  const savePolicy = async (next: PasswordPolicy, status: ReturnType<typeof useSaveStatus>): Promise<void> => {
    const before = policy
    setPolicy(next)
    try {
      const res = await userPost('/policy', next)
      if (!res.ok) {
        setPolicy(before)
        setMinDraft(String(before.minLength))
        status.failed(await errText(res))
        return
      }
      status.saved()
      // Re-read so a clamped minimum shows.
      void loadPolicy()
    } catch (e) {
      setPolicy(before)
      setMinDraft(String(before.minLength))
      status.failed(e)
    }
  }

  const minProblem = ((): string | null => {
    const n = Number(minDraft)
    return Number.isInteger(n) && n >= 4 && n <= 128 ? null : 'Use a whole number from 4 to 128.'
  })()

  const commitMin = (): void => {
    if (minProblem || Number(minDraft) === policy.minLength) return
    void savePolicy({ ...policy, minLength: Number(minDraft) }, minSave)
  }

  // ── Derived ───────────────────────────────────────────────────────────
  const sorted = useMemo(
    () => [...users].sort((a, b) => (ROLE_RANK[a.role ?? 'member'] ?? 9) - (ROLE_RANK[b.role ?? 'member'] ?? 9) || a.username.localeCompare(b.username)),
    [users],
  )
  const visible = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return sorted
    return sorted.filter((u) => u.username.includes(q) || (realName(u) ?? '').toLowerCase().includes(q) || roleLabel(u.role).toLowerCase().includes(q))
  }, [sorted, query])
  const ownerLogins = users.filter((u) => u.role === 'owner' && !u.disabled).length
  const roles = pickerOptions(who.role, voiceChatRole)
  const others = users.filter((u) => u.username !== who.username)

  // ── Gates: loading, can't read, not allowed ───────────────────────────
  if (!who.loaded) {
    return (
      <SettingsLoadState kind="loading" title="Loading the people on this server…" testId="admin-access-loading">
        Checking who can sign in and what each person can do.
      </SettingsLoadState>
    )
  }
  if (who.error) {
    return (
      <SettingsLoadState
        kind="error"
        title="Couldn’t check your role on this server"
        detail={who.error}
        action={<Button onClick={() => void loadAll()}>Try again</Button>}
        testId="admin-access-who-failed"
      >
        K2 needs your role to show who can sign in here. Try again, or check that this server is running.
      </SettingsLoadState>
    )
  }
  if (!canManage) {
    return oldHostNoRoles ? (
      <SettingsLoadState kind="empty" title="This server needs an update first" testId="admin-access-old-server">
        It runs K2 {serverVersion}. Managing people here needs K2 {featureMinVersion('roles')} or later on the server.
      </SettingsLoadState>
    ) : (
      <SettingsLoadState kind="empty" title="Only an Owner or Admin can change who signs in" testId="admin-access-not-admin">
        If someone needs to sign in to this server, or needs a different role, ask one of its Owners or Admins.
      </SettingsLoadState>
    )
  }

  const addButton = (
    <Button variant="accent" onClick={() => setDialog({ kind: 'add' })} disabled={roles.length === 0 && !isOwner}>
      Add person
    </Button>
  )

  return (
    <div className="space-y-6 pb-4" data-settings-id="k2-connect.users">
      {/* ── People ── */}
      {usersLoad === 'loading' ? (
        <SettingsLoadState kind="loading" title="Loading the people on this server…" testId="admin-access-loading">
          Checking who can sign in and what each person can do.
        </SettingsLoadState>
      ) : typeof usersLoad === 'object' ? (
        <SettingsLoadState
          kind="error"
          title="Couldn’t load the people on this server"
          detail={usersLoad.error}
          action={<Button onClick={() => { setUsersLoad('loading'); void loadUsers() }}>Try again</Button>}
          testId="admin-access-read-failed"
        >
          Try again in a moment. If it keeps failing, restart K2 on the server. No one’s access has changed.
        </SettingsLoadState>
      ) : (
        <SettingsGroup
          title="People"
          hint="Everyone who can sign in to this server, and what each person can do."
          action={others.length > 0 ? addButton : undefined}
          card
          testId="admin-people"
        >
          {notice ? (
            <div role="status" className="px-4 py-2.5 flex items-center justify-between gap-4" data-testid="admin-notice">
              <span className="inline-flex items-center gap-1.5 type-hint text-[var(--color-status-ok-soft)]">
                <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 flex-shrink-0">
                  <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
                </svg>
                {notice}
              </span>
              <Button variant="quiet" onClick={() => setNotice(null)}>
                Dismiss
              </Button>
            </div>
          ) : null}
          {users.length > 6 ? (
            <div className="px-4 py-2.5">
              <SearchField className="w-full" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search people" aria-label="Search people" />
            </div>
          ) : null}
          {visible.map((u) => (
            <PersonRow
              key={u.username}
              user={u}
              you={who.username === u.username}
              viewer={who.role}
              ownerToken={who.ownerToken}
              soleOwner={u.role === 'owner' && ownerLogins <= 1 && !who.ownerToken}
              roles={roles}
              voiceChatRole={voiceChatRole}
              status={rows.of(u.username)}
              ownerLinked={ownerLink !== 'hidden' && ownerAccounts.loaded && ownerAccounts.isLinked(u.username)}
              onRole={(r) => changeRole(u, r)}
              onSignIn={(on) => void setSignIn(u, on)}
              onClone={(on) => void setClone(u, on)}
              onRemove={() => setDialog({ kind: 'remove', user: u })}
              onReset={() => setDialog({ kind: 'reset', user: u })}
              onName={() => setDialog({ kind: 'name', user: u })}
              below={
                <>
                  {u.role === 'voice_chat' ? (
                    <VoiceChatSharesEditor username={u.username} shares={parseShares(u.shares)} onSave={(a, g) => saveShares(u.username, a, g)} />
                  ) : null}
                  {needsEnableAsMember(u) ? (
                    <WasViewerNotice username={u.username} busy={enableBusy === u.username} onEnableAsMember={(n) => void enableAsMember(n)} />
                  ) : null}
                </>
              }
            />
          ))}
          {query && visible.length === 0 ? (
            <SettingRow label="No one matches" hint={`Nobody’s name, username or role has “${query.trim()}” in it.`}>
              <Button variant="quiet" onClick={() => setQuery('')}>
                Clear search
              </Button>
            </SettingRow>
          ) : null}
          {others.length === 0 ? (
            <SettingRow
              label={users.length === 0 ? 'Only you so far' : 'No one else yet'}
              hint={
                users.length === 0
                  ? 'You own this server. Add a person to let them sign in from their own K2 app.'
                  : 'Add a person to let them sign in to this server from their own K2 app.'
              }
              testId="admin-only-you"
            >
              {addButton}
            </SettingRow>
          ) : null}
          {ownerAccounts.error ? (
            <div role="alert" className="px-4 py-2.5 type-hint leading-relaxed">
              <p className="text-[var(--color-status-error-soft)]">Couldn’t tell which of these logins is yours. Reopen this page to try again.</p>
              <p className="text-[var(--color-text-muted)] mt-1">The server said: {ownerAccounts.error.replace(/[.\s]+$/, '')}.</p>
            </div>
          ) : null}
        </SettingsGroup>
      )}

      {/* ── Password rules ── */}
      {policyLoad === 'loading' ? null : typeof policyLoad === 'object' ? (
        <SettingsLoadState
          kind="error"
          title="Couldn’t load the password rules"
          detail={policyLoad.error}
          action={<Button onClick={() => { setPolicyLoad('loading'); void loadPolicy() }}>Try again</Button>}
          testId="admin-policy-failed"
        >
          The rules still apply to every password. Try again to see or change them.
        </SettingsLoadState>
      ) : (
        <SettingsGroup
          title="Password rules"
          hint="What a password must have whenever someone sets or changes one."
          card
          settingId="k2-access.password-rules"
          testId="admin-password-rules"
        >
          <SettingRow
            label="Minimum length"
            hint={
              <>
                {minProblem ?? 'The fewest characters a password can have, from 4 to 128.'}
                <SaveError state={minSave.state} />
              </>
            }
          >
            <SaveNote state={minSave.state} />
            {/* In the people's role column: the field's edges line up
                with the role fields above, and the gap on the right is
                where their "…" menus sit. */}
            <div className={SETTING_FIELD_WIDTH}>
              <Input
                type="number"
                min={4}
                max={128}
                className="w-full type-control text-right"
                aria-label="Minimum length"
                aria-invalid={minProblem ? true : undefined}
                value={minDraft}
                onChange={(e) => setMinDraft(e.target.value)}
                onBlur={commitMin}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') commitMin()
                }}
              />
            </div>
            <span aria-hidden="true" className={MENU_SLOT} />
          </SettingRow>
          {(
            [
              ['requireNumber', 'Require a number', 'Such as 7.'],
              ['requireUppercase', 'Require an uppercase letter', 'Such as K.'],
              ['requireSpecial', 'Require a special character', 'Such as ! or #.'],
            ] as const
          ).map(([key, label, hint]) => (
            <SettingRow
              key={key}
              label={label}
              hint={
                <>
                  {hint}
                  <SaveError state={ruleSave[key].state} />
                </>
              }
            >
              <SaveNote state={ruleSave[key].state} />
              <Toggle checked={policy[key]} aria-label={label} onChange={(next) => void savePolicy({ ...policy, [key]: next }, ruleSave[key])} />
            </SettingRow>
          ))}
        </SettingsGroup>
      )}

      {/* Sew S32 K28: read-only chat state per login, only where its own
          chat grant exists. */}
      <SewAdminAccessState users={users} />

      {dialog?.kind === 'add' ? (
        <AddPersonDialog
          roles={roles}
          policy={policy}
          canSetHigherRoles={isOwner}
          serverAddress={serverAddress}
          taken={users.map((u) => u.username)}
          onClose={() => setDialog(null)}
          onAdded={() => void loadUsers()}
        />
      ) : dialog?.kind === 'reset' ? (
        <ResetPasswordDialog
          user={dialog.user}
          policy={policy}
          ownLogin={ownerAccounts.loaded && ownerAccounts.isLinked(dialog.user.username)}
          onClose={() => setDialog(null)}
          onDone={() => void loadUsers()}
        />
      ) : dialog?.kind === 'name' ? (
        <NameAndPictureDialog
          user={dialog.user}
          onClose={() => setDialog(null)}
          onSave={(f, l) => saveName(dialog.user.username, f, l)}
        />
      ) : dialog?.kind === 'remove' ? (
        <ConfirmSheet
          title={`Remove ${dialog.user.username} from this server?`}
          message={`${realName(dialog.user) ?? dialog.user.username} won’t be able to sign in here any more. You can add them again later.`}
          confirmLabel="Remove"
          destructive
          testId="remove-confirm-dialog"
          onCancel={() => setDialog(null)}
          onConfirm={() => {
            const u = dialog.user
            setDialog(null)
            void remove(u)
          }}
        />
      ) : dialog?.kind === 'owner' ? (
        <ConfirmSheet
          title={`Make ${dialog.user.username} an Owner?`}
          message="Owners can do everything on this server, including changing anyone’s role and removing people."
          confirmLabel="Make Owner"
          testId="owner-confirm-dialog"
          onCancel={() => setDialog(null)}
          onConfirm={() => {
            const u = dialog.user
            setDialog(null)
            void setRole(u, 'owner')
          }}
        />
      ) : null}
    </div>
  )
}

/** The width of a row's "…" menu button: the password rules keep it free
 *  so their field lines up with the role fields. */
const MENU_SLOT = 'w-8 flex-shrink-0'

/** A row change that failed: what, what to do now, then the server's
 *  reason quietly. */
function RowError({ fail }: { fail: RowFail }): React.JSX.Element {
  return (
    <div role="alert" data-save-error="" className={`${SETTING_ROW_HINT} mt-1`}>
      <p className="text-[var(--color-status-error-soft)]">
        {fail.what} {fail.next}
      </p>
      {fail.detail ? <p>The server said: {fail.detail.trim().replace(/[.\s]+$/, '')}.</p> : null}
    </div>
  )
}

/** One person: picture, name and username, what's special about them,
 *  their role in the shared field column, and the "…" menu. The picture,
 *  name and controls share the first line; notes and errors go under the
 *  name, so the role field never moves when a line is added. */
function PersonRow({
  user: u,
  you,
  viewer,
  ownerToken,
  soleOwner,
  roles,
  voiceChatRole,
  status,
  ownerLinked,
  onRole,
  onSignIn,
  onClone,
  onRemove,
  onReset,
  onName,
  below,
}: {
  user: K2User
  you: boolean
  viewer: K2Role | null
  ownerToken: boolean
  soleOwner: boolean
  roles: Array<{ value: K2Role; label: string }>
  voiceChatRole: boolean
  status: RowState
  ownerLinked: boolean
  onRole: (r: K2Role) => void
  onSignIn: (on: boolean) => void
  onClone: (on: boolean) => void
  onRemove: () => void
  onReset: () => void
  onName: () => void
  below: React.ReactNode
}): React.JSX.Element {
  const isOwnerViewer = viewer === 'owner'
  const rowRole = parseK2Role(u.role)
  const name = realName(u)
  // Why the role can't be changed here, or null when it can.
  const roleLock = you
    ? 'This is you. Another Owner can change your role.'
    : soleOwner
      ? 'The only Owner. Make someone else an Owner first.'
      : isOwnerViewer || (viewer === 'admin' && voiceChatRole && roles.some((r) => r.value !== rowRole && canSetRole(viewer, rowRole, r.value)))
        ? null
        : 'Only an Owner can change this role.'
  const roleChoices = roles.length > 0 ? roles : [{ value: (rowRole ?? 'member') as K2Role, label: roleLabel(u.role) }]
  const cloneLocked = u.role === 'owner' || !isOwnerViewer
  const canClone = u.role === 'owner' || u.canClone !== false

  // What the tags don't already say.
  const facts: string[] = []
  if (u.disabled) facts.push('Can’t sign in until you turn sign-in back on')
  if (!canClone) facts.push('Can’t clone agents to their own computer')

  const lines = [...((you || soleOwner) && roleLock ? [roleLock] : []), ...facts]
  const fail = typeof status === 'object' ? status : null

  const items: OverflowMenuItem[] = [
    { id: 'name', label: 'Name and picture…', onSelect: onName },
    {
      id: 'reset',
      label: 'Reset password…',
      onSelect: onReset,
      disabled: !isOwnerViewer && !ownerToken,
      hint: !isOwnerViewer && !ownerToken ? 'Only an Owner can reset passwords.' : undefined,
    },
    {
      id: 'signin',
      label: u.disabled ? 'Turn on sign-in' : 'Turn off sign-in',
      hint: you ? 'You can’t turn off your own sign-in.' : u.disabled ? 'They can sign in again.' : 'They can’t sign in until you turn it back on.',
      disabled: you && !u.disabled,
      onSelect: () => onSignIn(u.disabled),
    },
    {
      id: 'clone',
      label: canClone ? 'Stop them cloning agents' : 'Let them clone agents',
      hint: cloneLocked
        ? u.role === 'owner'
          ? 'Owners can always clone agents.'
          : 'Only an Owner can change this.'
        : 'Cloning copies an agent from this server to their own computer.',
      disabled: cloneLocked,
      onSelect: () => onClone(!canClone),
    },
    {
      id: 'remove',
      label: 'Remove from this server…',
      danger: true,
      group: true,
      disabled: you || soleOwner || (!isOwnerViewer && !ownerToken),
      hint: you
        ? 'You can’t remove yourself.'
        : soleOwner
          ? 'The only Owner can’t be removed.'
          : !isOwnerViewer && !ownerToken
            ? 'Only an Owner can remove people.'
            : undefined,
      onSelect: onRemove,
    },
  ]

  return (
    <div className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`person-row-${u.username}`}>
      <div className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3">
        {/* No role mark on the picture: the role field says it. */}
        <span data-testid={`user-chip-${u.username}`} className="flex flex-none">
          <PresenceChip name={u.username} role="member" size={28} color={u.color ?? null} names={u} pictureScope={primaryScope()} title="" away={u.disabled} />
        </span>
        <div className="flex items-center gap-2 min-w-0 flex-wrap">
          {name ? (
            <span className="type-name text-[var(--color-text-primary)] truncate" data-testid={`user-real-name-${u.username}`}>
              {name}
            </span>
          ) : null}
          <span className={`${name ? 'type-hint text-[var(--color-text-secondary)]' : 'type-name text-[var(--color-text-primary)]'} font-mono truncate`}>{u.username}</span>
          {you ? <Tag tone="accent">You</Tag> : null}
          {ownerLinked ? <OwnerAccountTag username={u.username} linked /> : null}
          {u.disabled ? <Tag tone="muted">Sign-in off</Tag> : null}
          {u.wasViewer ? <WasViewerBadge /> : null}
          {u.mustChangePassword === true ? (
            <Tag tone="warn" data-testid={`temporary-password-${u.username}`} title={MUST_CHANGE_NOTE}>
              Temporary password
            </Tag>
          ) : null}
        </div>
        <div className="flex items-center gap-2 pl-3">
          <SaveNote state={status === 'saved' ? 'saved' : 'idle'} />
          <div className={SETTING_FIELD_WIDTH} title={roleLock ?? undefined}>
            <SettingDropdown
              fullWidth
              value={u.role ?? 'member'}
              placeholder={roleLabel(u.role)}
              options={roleChoices.map((r) => ({ ...r, disabled: viewer === 'admin' && !canSetRole(viewer, rowRole, r.value) }))}
              ariaLabel={`Role for ${u.username}`}
              disabled={roleLock !== null}
              onChange={(value) => {
                const r = parseK2Role(value)
                if (r) onRole(r)
              }}
            />
          </div>
          <OverflowMenu label={`More actions for ${u.username}`} items={items} />
        </div>
        {lines.length > 0 || fail ? (
          <div className="col-start-2 min-w-0">
            {lines.length > 0 ? <p className={SETTING_ROW_HINT}>{lines.join(' · ')}</p> : null}
            {fail ? <RowError fail={fail} /> : null}
          </div>
        ) : null}
      </div>
      {below ? <div className="pl-10 empty:hidden">{below}</div> : null}
    </div>
  )
}
