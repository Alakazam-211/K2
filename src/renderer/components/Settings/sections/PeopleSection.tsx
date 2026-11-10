// Settings → Access → User Access: who can use this server, and what each
// person can use. A list and detail like Workspaces / Agents: the list on
// the left (Everyone, then each person), and on the right either everyone
// at a glance or one person, in two tabs (What they can use, Account).
//
// The daemon's words (principals, grants) stay in user-access-api.ts; the
// page says "people" and "what they can use". Templates live under User
// Templates; logins for K2 itself under Admin Access (Connect users).

import React, { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import { primaryScope } from '@/kessel/server-scope'
import { Button, Input, OverflowMenu, Surface, Tag, Toggle, cx } from '@/components/ui'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useSettingsStore } from '@/stores/settings'
import {
  SETTING_FIELD_WIDTH,
  SETTING_ROW_HINT,
  SaveError,
  SaveNote,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  useSaveStatus,
  type SaveState,
} from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { AccessAuditLink } from '../shared/AccessAuditLink'
import { SewPersonAccess } from './SewAccessParts'
import type { ChatGrant } from './sew-api'
import {
  AddUserDialog,
  AddedUserDialog,
  ResetPasswordDialog,
  SetPasswordDialog,
  type AddedUser,
} from './AppUserDialogs'
import {
  HOST_APP,
  HOST_LABEL,
  appName,
  fetchApps,
  parseGrants,
  parsePeople,
  parseRoles,
  reachWords,
  roleLabel,
  takePersonHandoff,
  type AppOption,
  type GrantRow,
  type PersonRow,
  type RoleRow,
} from './user-access-api'

export { parsePeople }
export type { PersonRow }

export const PEOPLE_MANIFEST: SettingEntry[] = [
  {
    id: 'people.roster',
    section: 'people',
    label: 'User Access',
    description: 'Who can use this server’s K2 page, its Apps and chat, and what each person reaches.',
    keywords: ['people', 'user access', 'full name', 'principal', 'guest', 'username', 'grant', 'password', 'app', 'unlock', 'reset password'],
    group: 'People',
  },
]

/** Why an Admin's password actions are off on a box login's row. */
export const BOX_LOGIN_REASON = 'This person also signs in to K2 itself, not only its Apps, so only the owner can change their password or unlock them.'

/**
 * The daemon's rule (0.46.6, `owner_or_admin_for`): the Owner resets,
 * sets and unlocks anyone; an Admin only people who use Apps alone, never
 * a box login. Anyone else: nothing.
 */
export function canResetPassword(role: 'owner' | 'admin' | null, row: Pick<PersonRow, 'boxLogin'>): boolean {
  if (role === 'owner') return true
  return role === 'admin' && !row.boxLogin
}

type Load = 'loading' | 'ready' | { error: string }
type Selection = 'everyone' | string
type ViewerRole = 'owner' | 'admin' | null

/** One write at a time on a page: which row it was, whether it's running,
 *  and how it ended (the shared "Saved" note, or why it failed). */
function useRowWrites(): {
  busy: string | null
  at: (key: string) => SaveState
  run: (key: string, fn: () => Promise<unknown>) => Promise<boolean>
} {
  const status = useSaveStatus()
  const [key, setKey] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  return {
    busy,
    at: (k) => (key === k ? status.state : 'idle'),
    run: async (k, fn) => {
      setKey(k)
      setBusy(k)
      status.reset()
      try {
        await fn()
        status.saved()
        return true
      } catch (e) {
        status.failed(new Error(daemonErrorText(e)))
        return false
      } finally {
        setBusy(null)
      }
    },
  }
}

const displayName = (p: PersonRow): string => p.fullName ?? p.username
/** The line under a name: the username when the name is a full name, else
 *  the email, so a row never says the same word twice. */
const secondLine = (p: PersonRow): string => (p.fullName ? p.username : p.email ?? 'No full name yet')

/** The person's tags: locked out, or waiting for their own password. */
function PersonTags({ person }: { person: PersonRow }): React.JSX.Element | null {
  if (!person.lockedUntil && !person.pendingReset) return null
  return (
    <span className="flex items-center gap-1.5 flex-shrink-0">
      {person.pendingReset ? <Tag tone="warn">Waiting for password</Tag> : null}
      {person.lockedUntil ? <Tag tone="danger" title="Locked after too many wrong passwords">Locked</Tag> : null}
    </span>
  )
}

/** What a person can use, in a few words, for the list of everyone. */
function accessSummary(person: PersonRow, grants: GrantRow[], apps: AppOption[] | null): string {
  if (!person.id) return 'Nothing yet'
  const mine = grants.filter((g) => g.subjectKind === 'principal' && g.subjectId === person.id && g.enabled)
  const parts: string[] = []
  if (mine.some((g) => g.kind === 'app' && g.targetId === HOST_APP)) parts.push(HOST_LABEL)
  for (const g of mine) if (g.kind === 'app' && g.targetId !== HOST_APP) parts.push(appName(g.targetId, apps))
  const chat = mine.find((g) => g.kind === 'chat')
  if (chat) parts.push(chat.scope === 'guest' ? 'Chat as a guest' : 'Chat')
  return parts.length ? parts.join(' · ') : 'Nothing yet'
}

function PeopleIcon(): React.JSX.Element {
  return (
    <svg className="w-4 h-4 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.75} aria-hidden="true">
      <path strokeLinecap="round" strokeLinejoin="round" d="M17 20h5v-2a3 3 0 00-5.356-1.857M17 20H7m10 0v-2c0-.656-.126-1.283-.356-1.857M7 20H2v-2a3 3 0 015.356-1.857M7 20v-2c0-.656.126-1.283.356-1.857m0 0a5.002 5.002 0 019.288 0M15 7a3 3 0 11-6 0 3 3 0 016 0z" />
    </svg>
  )
}

function Chevron(): React.JSX.Element {
  return (
    <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-text-secondary)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2} aria-hidden="true">
      <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
    </svg>
  )
}

export function PeopleSection(): React.JSX.Element {
  const [people, setPeople] = useState<PersonRow[]>([])
  const [roles, setRoles] = useState<RoleRow[]>([])
  const [grants, setGrants] = useState<GrantRow[]>([])
  const [load, setLoad] = useState<Load>('loading')
  const [apps, setApps] = useState<AppOption[] | null>(null)
  const [appsError, setAppsError] = useState<string | null>(null)
  const [selection, setSelection] = useState<Selection>(() => takePersonHandoff() ?? 'everyone')
  // 0.46.6: the daemon lets the Owner manage every App user's password
  // and an Admin those of people who only use Apps (never a box login);
  // adding a person stays the Owner's. Nobody sees actions that would fail.
  const [viewerRole, setViewerRole] = useState<ViewerRole>(null)
  const isOwner = viewerRole === 'owner'
  const [dialog, setDialog] = useState<{ kind: 'reset' | 'set'; username: string } | null>(null)
  const [adding, setAdding] = useState(false)
  // The one-time result of Add person; dropped when its sheet closes.
  const [added, setAdded] = useState<AddedUser | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  useEffect(() => {
    let live = true
    void fetchViewerRole().then((role) => {
      if (live) setViewerRole(role === 'owner' || role === 'admin' ? role : null)
    })
    return () => {
      live = false
    }
  }, [])

  /** Re-reads everything. The first read shows the loading card; later ones
   *  keep the page and only report a failure. */
  const refresh = useCallback(async () => {
    try {
      const [u, g, r] = await Promise.all([
        daemonCliGet<unknown>(primaryScope(), 'skin/users'),
        daemonCliGet<unknown>(primaryScope(), 'skin/grants'),
        daemonCliGet<unknown>(primaryScope(), 'skin/roles'),
      ])
      setPeople(parsePeople(u))
      setGrants(parseGrants(g))
      setRoles(parseRoles(r))
      setLoad('ready')
    } catch (e) {
      setLoad({ error: daemonErrorText(e) })
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  /** App names: read once, the first time a page shows an App. */
  const loadApps = useCallback(async () => {
    if (apps) return
    setAppsError(null)
    try {
      setApps(await fetchApps())
    } catch (e) {
      setAppsError(daemonErrorText(e))
    }
  }, [apps])

  const showsApps = grants.some((g) => g.kind === 'app' && g.targetId !== HOST_APP)
  useEffect(() => {
    if (showsApps) void loadApps()
  }, [showsApps, loadApps])

  const person = selection !== 'everyone' ? people.find((p) => p.username === selection) ?? null : null
  const dialogPerson = dialog ? people.find((p) => p.username === dialog.username) ?? null : null
  const postHere = (route: string, body: Record<string, unknown>): Promise<unknown> => daemonCliPost(primaryScope(), route, body)
  // Closing a sheet drops whatever it showed once, and re-reads the list
  // (a reset or set can change the lock and the tie).
  const closeDialog = (): void => {
    setDialog(null)
    void refresh()
  }
  const select = (next: Selection): void => {
    setSelection(next)
    setNotice(null)
  }

  const ready = load === 'ready'
  const roleChoices = roles.map((r) => ({ value: r.id, label: r.name, appId: r.appId }))
  const appChoices = (apps ?? []).map((a) => ({ value: a.id, label: `${a.name} (${a.kind === 'skin' ? 'app' : a.kind})` }))

  return (
    <div className="h-full min-h-0 flex">
      <aside aria-label="People" className="w-72 flex-shrink-0 min-h-0 flex flex-col border-r border-[var(--color-border)]">
        <div className="flex-shrink-0 px-4 pt-6 pb-3">
          <h2 className="flex items-center h-7 type-heading text-[var(--color-text-primary)]">User Access</h2>
          <p className="type-hint text-[var(--color-text-secondary)] leading-relaxed">
            Who can use this server, and what each person can use.
          </p>
          <AccessAuditLink className="mt-1" />
        </div>
        <div data-settings-id="people.roster" className="flex-1 min-h-0 overflow-y-auto px-4 pb-4 space-y-4 [scrollbar-gutter:stable]">
          <button
            type="button"
            data-testid="people-everyone"
            aria-current={selection === 'everyone' ? 'true' : undefined}
            onClick={() => select('everyone')}
            data-k2-chamfer="control"
            className={cx(
              'k2-focus relative w-full flex items-center gap-3 px-3 py-2 rounded-[var(--radius-selector)] text-left type-control no-drag cursor-pointer transition-colors',
              selection === 'everyone'
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-wash-1)]',
            )}
          >
            <PeopleIcon />
            <span className="flex-1 truncate">Everyone</span>
            {ready ? <span className="type-hint text-[var(--color-text-secondary)] tabular-nums">{people.length}</span> : null}
          </button>
          {ready && people.length > 0 ? (
            <section>
              <div className="flex items-center h-7 mb-1 type-heading text-[var(--color-text-primary)]">People</div>
              {/* overflow-hidden: a selected or hovered row keeps to the card's corners. */}
              <Surface chamfer material="tile" className="overflow-hidden">
                <PersonList people={people} selection={selection} onSelect={select} />
              </Surface>
            </section>
          ) : null}
        </div>
      </aside>

      <div className="flex-1 min-w-0 min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]">
        <div className="w-full max-w-[52rem] space-y-6 pb-6">
          {load === 'loading' ? (
            <>
              <EveryoneHeader isOwner={false} onAdd={() => setAdding(true)} waiting />
              <SettingsLoadState kind="loading" title="Loading people…" testId="people-loading">
                Reading who can use this server.
              </SettingsLoadState>
            </>
          ) : typeof load === 'object' ? (
            <>
              <EveryoneHeader isOwner={false} onAdd={() => setAdding(true)} waiting />
              <SettingsLoadState
                kind="error"
                title="Couldn’t load people"
                detail={load.error}
                testId="people-error"
                action={<Button onClick={() => { setLoad('loading'); void refresh() }}>Try again</Button>}
              >
                K2 couldn’t read who can use this server. Nothing has changed.
              </SettingsLoadState>
            </>
          ) : person ? (
            <PersonDetail
              key={person.username}
              person={person}
              grants={grants}
              roles={roles}
              apps={apps}
              appsError={appsError}
              viewerRole={viewerRole}
              loadApps={loadApps}
              refresh={refresh}
              onDialog={(kind) => setDialog({ kind, username: person.username })}
              onRemoved={(username) => {
                setSelection('everyone')
                setNotice(`${username} was removed. They can no longer sign in here.`)
              }}
            />
          ) : (
            <Everyone
              people={people}
              grants={grants}
              apps={apps}
              isOwner={isOwner}
              notice={notice}
              onAdd={() => setAdding(true)}
              onOpen={select}
            />
          )}
        </div>
      </div>

      {dialogPerson && dialog?.kind === 'reset' ? (
        <ResetPasswordDialog key={`reset:${dialogPerson.username}`} person={dialogPerson} onClose={closeDialog} post={postHere} />
      ) : null}
      {dialogPerson && dialog?.kind === 'set' ? (
        <SetPasswordDialog
          key={`set:${dialogPerson.username}`}
          person={dialogPerson}
          onClose={closeDialog}
          post={postHere}
          onReset={() => setDialog({ kind: 'reset', username: dialogPerson.username })}
        />
      ) : null}
      {adding ? (
        <AddUserDialog
          onClose={() => setAdding(false)}
          apps={appChoices}
          roles={roleChoices}
          loadApps={loadApps}
          post={postHere}
          onAdded={(result) => {
            setAdding(false)
            setSelection(result.username)
            if (result.secret || result.note) setAdded(result)
            void refresh()
          }}
        />
      ) : null}
      {added ? <AddedUserDialog added={added} onClose={() => setAdded(null)} /> : null}
    </div>
  )
}

/** The list column's people: name, username, and any tag. */
function PersonList({
  people,
  selection,
  onSelect,
}: {
  people: PersonRow[]
  selection: Selection
  onSelect: (username: string) => void
}): React.JSX.Element {
  return (
    <div className="divide-y divide-[var(--color-border)]">
      {people.map((row) => {
        const selected = selection === row.username
        return (
          <button
            key={row.username}
            type="button"
            aria-label={`Open ${row.username}`}
            aria-current={selected ? 'true' : undefined}
            data-person-row={row.username}
            data-k2-focus=""
            onClick={() => onSelect(row.username)}
            className={cx(
              'relative w-full flex items-center gap-3 px-3 py-2 text-left no-drag cursor-pointer transition-colors outline-none',
              selected ? 'bg-[var(--color-accent)]/15' : 'hover:bg-[var(--color-wash-1)]',
            )}
          >
            {selected && <span aria-hidden="true" className="absolute left-0 top-0 bottom-0 w-[3px] bg-[var(--color-accent)]" />}
            <span className="min-w-0 flex-1">
              <span className="block type-name text-[var(--color-text-primary)] truncate">{displayName(row)}</span>
              <span className="flex items-center gap-2 min-w-0">
                <span className="type-hint text-[var(--color-text-secondary)] truncate">{secondLine(row)}</span>
                <PersonTags person={row} />
              </span>
            </span>
          </button>
        )
      })}
    </div>
  )
}

function EveryoneHeader({ isOwner, onAdd, waiting = false }: { isOwner: boolean; onAdd: () => void; waiting?: boolean }): React.JSX.Element {
  return (
    <header className="flex items-start justify-between gap-4">
      <div className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)]">Everyone</h2>
        <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed">
          {waiting ? 'What each person can use on this server.' : 'What each person can use on this server. Pick someone to change it.'}
        </p>
      </div>
      {isOwner ? (
        <Button variant="accent" className="flex-shrink-0" onClick={onAdd}>
          Add person
        </Button>
      ) : null}
    </header>
  )
}

/** Everyone at a glance: each person and what they can use, or how people
 *  get here when there's no one yet. */
function Everyone({
  people,
  grants,
  apps,
  isOwner,
  notice,
  onAdd,
  onOpen,
}: {
  people: PersonRow[]
  grants: GrantRow[]
  apps: AppOption[] | null
  isOwner: boolean
  notice: string | null
  onAdd: () => void
  onOpen: (username: string) => void
}): React.JSX.Element {
  if (people.length === 0) {
    return (
      <>
        <EveryoneHeader isOwner={false} onAdd={onAdd} waiting />
        <SettingsLoadState
          kind="empty"
          title="No one can use this server yet"
          testId="people-empty"
          action={
            <>
              <Button variant="quiet" onClick={() => useSettingsStore.getState().setSection('k2-access')}>
                Open Admin Access
              </Button>
              {isOwner ? (
                <Button variant="accent" onClick={onAdd}>
                  Add person
                </Button>
              ) : null}
            </>
          }
        >
          People show up here when you add them, when they accept an invite to one of your Apps, or when an agent sets them up
          for its App. To let someone run K2 itself, use Admin Access.
        </SettingsLoadState>
      </>
    )
  }
  return (
    <>
      <EveryoneHeader isOwner={isOwner} onAdd={onAdd} />
      {notice ? (
        <p role="status" className="type-hint text-[var(--color-status-ok-soft)]">
          {notice}
        </p>
      ) : null}
      <section data-testid="people-overview">
        <div className="mb-2">
          <h3 className="flex items-center min-h-[1.5rem] type-heading text-[var(--color-text-primary)]">People</h3>
          <p className={SETTING_ROW_HINT}>Where each person can sign in. Pick someone to see more or change it.</p>
        </div>
        {/* overflow-hidden: a hovered row keeps to the card's corners. */}
        <Surface chamfer material="tile" className="overflow-hidden divide-y divide-[var(--color-border)]">
        {people.map((row) => (
          <button
            key={row.username}
            type="button"
            aria-label={`See what ${row.username} can use`}
            data-overview-row={row.username}
            data-k2-focus=""
            onClick={() => onOpen(row.username)}
            className="relative w-full flex items-center gap-4 px-4 py-3 text-left no-drag cursor-pointer transition-colors outline-none hover:bg-[var(--color-wash-1)]"
          >
            <span className="min-w-0 w-56 flex-shrink-0">
              <span className="block type-name text-[var(--color-text-primary)] truncate">{displayName(row)}</span>
              <span className="flex items-center gap-2 min-w-0">
                <span className="type-hint text-[var(--color-text-secondary)] truncate">{secondLine(row)}</span>
                <PersonTags person={row} />
              </span>
            </span>
            <span className="min-w-0 flex-1 type-row text-[var(--color-text-secondary)] truncate" data-testid={`summary-${row.username}`}>
              {accessSummary(row, grants, apps)}
            </span>
            <Chevron />
          </button>
        ))}
        </Surface>
      </section>
    </>
  )
}

type Tab = 'access' | 'account'
const TAB_LABEL: Record<Tab, string> = { access: 'What they can use', account: 'Account' }

function PersonDetail({
  person,
  grants,
  roles,
  apps,
  appsError,
  viewerRole,
  loadApps,
  refresh,
  onDialog,
  onRemoved,
}: {
  person: PersonRow
  grants: GrantRow[]
  roles: RoleRow[]
  apps: AppOption[] | null
  appsError: string | null
  viewerRole: ViewerRole
  loadApps: () => Promise<void>
  refresh: () => Promise<void>
  onDialog: (kind: 'reset' | 'set') => void
  onRemoved: (username: string) => void
}): React.JSX.Element {
  const [tab, setTab] = useState<Tab>('access')
  const writes = useRowWrites()
  const canReset = canResetPassword(viewerRole, person)
  const mine = person.id ? grants.filter((g) => g.subjectKind === 'principal' && g.subjectId === person.id) : []
  const chatGrant = mine.find((g) => g.kind === 'chat')
  const sewGrant: ChatGrant | null = chatGrant
    ? { id: chatGrant.id, subjectKind: chatGrant.subjectKind, subjectId: chatGrant.subjectId, scope: chatGrant.scope, enabled: chatGrant.enabled, hookState: chatGrant.hookState, hookError: chatGrant.hookError }
    : null

  return (
    <>
      <header className="space-y-4">
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="flex items-center gap-2 min-w-0">
              <h2 className="type-title text-[var(--color-text-primary)] truncate">{displayName(person)}</h2>
              <PersonTags person={person} />
            </div>
            <p className="type-hint text-[var(--color-text-secondary)] mt-1 truncate">
              {[person.fullName ? person.username : null, person.email].filter(Boolean).join(' · ') || 'No full name or email yet'}
            </p>
          </div>
        </div>
        <div role="tablist" aria-label={`${person.username}’s settings`} className="flex flex-wrap gap-0.5 border-b border-[var(--color-border)] w-full">
          {(Object.keys(TAB_LABEL) as Tab[]).map((id) => {
            const active = tab === id
            return (
              <button
                key={id}
                role="tab"
                type="button"
                aria-selected={active}
                onClick={() => setTab(id)}
                data-k2-focus=""
                className={cx(
                  'relative px-3 py-2 type-control transition-colors no-drag cursor-pointer border-b-2 -mb-px outline-none',
                  active
                    ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                    : 'border-transparent text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]',
                )}
              >
                {TAB_LABEL[id]}
              </button>
            )
          })}
        </div>
      </header>

      {tab === 'access' ? (
        <>
          {person.tiedTo ? <ResetPanel person={person} onReset={canReset ? () => onDialog('reset') : null} /> : null}
          <SignInCard person={person} grants={mine} roles={roles} apps={apps} appsError={appsError} loadApps={loadApps} refresh={refresh} writes={writes} />
          <SewPersonAccess principalId={person.id} username={person.username} grant={sewGrant} onChanged={refresh} />
          <NotedCard person={person} grants={mine} refresh={refresh} writes={writes} />
        </>
      ) : (
        <AccountTab person={person} viewerRole={viewerRole} canReset={canReset} refresh={refresh} writes={writes} onDialog={onDialog} onRemoved={onRemoved} />
      )}
    </>
  )
}

/**
 * agent-app-users §5.5: a person an agent set up is tied to that agent's
 * workspace. When you gave them more, the extra waits until they choose their
 * own password, through the App or the owner's Reset password (a one-time link,
 * or mailed to an address you give). The stored email is never used for the
 * owner's reset: an agent could have chosen it. Nothing for an untied person.
 * `onReset` opens the Reset password sheet (the owner, or an Admin for someone
 * who only uses Apps; null hides it).
 */
export function ResetPanel({ person, onReset }: { person: PersonRow; onReset: (() => void) | null }): React.JSX.Element | null {
  if (!person.tiedTo) return null
  const agent = person.tiedTo
  return (
    <SettingsGroup title={person.pendingReset ? 'Waiting for their own password' : 'Set up by an agent'} card testId="person-tie">
      <SettingRow
        label={`The ${agent} agent set ${person.username} up`}
        hint={
          <>
            {person.pendingReset
              ? `The extra access you gave starts once they choose their own password. Until then they reach only ${agent}.`
              : `They reach only ${agent} until they choose their own password, in the App or from a reset link you send. Anything more you give waits until then.`}{' '}
            The agent may have entered their email, so K2 never sends your reset there: send it to an address you know is
            theirs, or get a one-time link.
          </>
        }
      >
        {onReset ? <Button onClick={onReset}>Reset password…</Button> : null}
      </SettingRow>
    </SettingsGroup>
  )
}

type Writes = ReturnType<typeof useRowWrites>

/** Where they can sign in: this server's K2 page, then each App, each with
 *  the role that sets which agents they reach. */
function SignInCard({
  person,
  grants,
  roles,
  apps,
  appsError,
  loadApps,
  refresh,
  writes,
}: {
  person: PersonRow
  grants: GrantRow[]
  roles: RoleRow[]
  apps: AppOption[] | null
  appsError: string | null
  loadApps: () => Promise<void>
  refresh: () => Promise<void>
  writes: Writes
}): React.JSX.Element {
  const [adding, setAdding] = useState(false)
  const [pickApp, setPickApp] = useState('')
  const [pickRole, setPickRole] = useState('')
  const host = grants.find((g) => g.kind === 'app' && g.targetId === HOST_APP)
  const appGrants = grants.filter((g) => g.kind === 'app' && g.targetId !== HOST_APP)
  const roleOf = (id: string | null): RoleRow | null => (id ? roles.find((r) => r.id === id) ?? null : null)
  const hostRole = roleOf(host?.roleId ?? person.roleId)
  const first = person.fullName?.split(' ')[0] ?? person.username

  const setHost = async (on: boolean): Promise<void> => {
    if (!on) {
      const ok = await useConfirmDialogStore.getState().confirm({
        title: `Turn off ${HOST_LABEL} for ${person.username}?`,
        message: `${person.username} is signed out of this server’s K2 page and can’t sign in again until you turn it back on. Their Apps aren’t changed.`,
        confirmLabel: 'Turn off',
        destructive: true,
      })
      if (!ok) return
    }
    if (await writes.run('host', () => daemonCliPost(primaryScope(), 'skin/grants/host', { username: person.username, present: on }))) await refresh()
  }
  const setEnabled = async (grant: GrantRow, on: boolean): Promise<void> => {
    if (await writes.run(grant.id, () => daemonCliPost(primaryScope(), 'skin/grants/enabled', { id: grant.id, enabled: on }))) await refresh()
  }
  const remove = async (grant: GrantRow): Promise<void> => {
    const name = appName(grant.targetId, apps)
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Remove ${person.username}’s access to ${name}?`,
      message: `${person.username} is signed out of ${name} and loses the role you gave them there. You can give it back later.`,
      confirmLabel: 'Remove access',
      destructive: true,
    })
    if (!ok) return
    if (await writes.run('apps', () => daemonCliPost(primaryScope(), 'skin/grants/delete', { id: grant.id }))) await refresh()
  }
  const add = async (): Promise<void> => {
    if (!person.id || !pickApp || !pickRole) return
    const done = await writes.run('apps', () =>
      daemonCliPost(primaryScope(), 'skin/grants', { subjectKind: 'principal', subjectId: person.id, kind: 'app', targetId: pickApp, roleId: pickRole }),
    )
    if (done) {
      setAdding(false)
      setPickApp('')
      setPickRole('')
      await refresh()
    }
  }

  const openForm = (): void => {
    setAdding(true)
    void loadApps()
  }
  const free = (apps ?? []).filter((a) => !appGrants.some((g) => g.targetId === a.id))
  const roleChoices = roles
    .filter((r) => !r.appId || !pickApp || r.appId === pickApp)
    .map((r) => ({ value: r.id, label: roleLabel(r, apps) }))

  return (
    <SettingsGroup
      title="Sign-in and Apps"
      hint={`Where ${first} can sign in, and the role that sets which agents they reach there.`}
      card
      testId="person-sign-in"
      action={
        <span className="flex items-center gap-2">
          <SaveNote state={writes.at('apps')} />
          {!adding ? (
            <Button onClick={openForm} disabled={!person.id}>
              Add an App
            </Button>
          ) : null}
        </span>
      }
    >
      <SettingRow
        label={HOST_LABEL}
        hint={host ? `Signs in to this server’s own K2 page. ${reachWords(hostRole, person.defaultRoomHandles)}` : 'Off. They can’t sign in to this server’s own K2 page.'}
        below={<SaveError state={writes.at('host')} />}
        testId="person-host"
      >
        <SaveNote state={writes.at('host')} />
        <Toggle
          checked={Boolean(host)}
          aria-label={`${HOST_LABEL} for ${person.username}`}
          disabled={writes.busy === 'host'}
          onChange={(on) => void setHost(on)}
        />
      </SettingRow>
      {appGrants.map((grant) => {
        const name = appName(grant.targetId, apps)
        return (
          <SettingRow
            key={grant.id}
            label={name}
            hint={grant.enabled ? reachWords(roleOf(grant.roleId)) : `Off. ${reachWords(roleOf(grant.roleId)).replace(/\.$/, '')} when turned on.`}
            below={<SaveError state={writes.at(grant.id)} />}
            testId={`person-app-${grant.targetId}`}
          >
            <SaveNote state={writes.at(grant.id)} />
            {/* Before the switch, so every switch in the card shares one edge. */}
            <OverflowMenu
              label={`More actions for ${name}`}
              items={[{ id: 'remove', label: 'Remove access…', danger: true, onSelect: () => void remove(grant) }]}
            />
            <Toggle
              checked={grant.enabled}
              aria-label={`${name} for ${person.username}`}
              disabled={writes.busy === grant.id}
              onChange={(on) => void setEnabled(grant, on)}
            />
          </SettingRow>
        )
      })}
      {appGrants.length === 0 && !adding ? (
        <SettingRow label="No Apps yet" hint={`Add an App to let ${first} sign in to it.`}>
          {null}
        </SettingRow>
      ) : null}
      {adding ? (
        <div data-testid="person-add-app">
          <SettingRow
            label="App"
            hint={appsError ? undefined : apps && free.length === 0 ? 'Every App on this server is already listed above.' : 'Published Apps on this server.'}
            below={appsError ? <p role="alert" className="type-hint text-[var(--color-status-error-soft)]">Couldn’t list the Apps: {appsError.replace(/[.\s]+$/, '')}. Try again in a moment.</p> : undefined}
            field
          >
            <SettingDropdown
              ariaLabel="App to add"
              value={pickApp}
              placeholder={apps ? 'Choose an App' : 'Loading Apps…'}
              disabled={!apps || free.length === 0}
              fullWidth
              options={free.map((a) => ({ value: a.id, label: a.name }))}
              onChange={(v) => {
                setPickApp(v)
                const role = roleOf(pickRole)
                if (role?.appId && role.appId !== v) setPickRole('')
              }}
            />
          </SettingRow>
          <SettingRow
            label="Role"
            hint={roles.length ? 'Sets which agents they reach in this App.' : 'No roles yet. Make one in Settings → Apps first.'}
            field
          >
            <SettingDropdown
              ariaLabel="Role in the App"
              value={pickRole}
              placeholder="Choose a role"
              disabled={roleChoices.length === 0}
              fullWidth
              options={roleChoices}
              onChange={setPickRole}
            />
          </SettingRow>
          <div className="px-4 py-3 flex items-center justify-end gap-2">
            <Button variant="quiet" onClick={() => setAdding(false)}>
              Cancel
            </Button>
            <Button variant="accent" disabled={!pickApp || !pickRole || writes.busy === 'apps'} onClick={() => void add()}>
              {writes.busy === 'apps' ? 'Adding…' : 'Add App'}
            </Button>
          </div>
        </div>
      ) : null}
      {typeof writes.at('apps') === 'object' ? (
        <div className="px-4 py-3">
          <SaveError state={writes.at('apps')} />
        </div>
      ) : null}
    </SettingsGroup>
  )
}

const NOTED_KIND: Record<string, string> = { database: 'Database', cli: 'Command-line tool' }

/** Database and command-line access: K2 keeps these as notes and doesn't
 *  enforce them, so the card says so and offers only removal. Shown only
 *  when someone has one. */
function NotedCard({
  person,
  grants,
  refresh,
  writes,
}: {
  person: PersonRow
  grants: GrantRow[]
  refresh: () => Promise<void>
  writes: Writes
}): React.JSX.Element | null {
  const noted = grants.filter((g) => g.kind in NOTED_KIND)
  if (noted.length === 0) return null
  const remove = async (grant: GrantRow): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Remove the note about ${grant.targetId}?`,
      message: `K2 forgets that ${person.username} may use ${grant.targetId}. Nothing else changes.`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (!ok) return
    if (await writes.run(grant.id, () => daemonCliPost(primaryScope(), 'skin/grants/delete', { id: grant.id }))) await refresh()
  }
  return (
    <SettingsGroup title="Databases and tools" hint="Kept as a note for your records. K2 doesn’t enforce these." card testId="person-noted">
      {noted.map((grant) => (
        <SettingRow
          key={grant.id}
          label={`${NOTED_KIND[grant.kind]}: ${grant.targetId}`}
          hint={grant.scope ? `Noted access: ${grant.scope}` : 'Noted only'}
          below={<SaveError state={writes.at(grant.id)} />}
        >
          <Tag tone="muted">Noted only</Tag>
          <OverflowMenu label={`More actions for ${grant.targetId}`} items={[{ id: 'remove', label: 'Remove note…', danger: true, onSelect: () => void remove(grant) }]} />
        </SettingRow>
      ))}
    </SettingsGroup>
  )
}

function AccountTab({
  person,
  viewerRole,
  canReset,
  refresh,
  writes,
  onDialog,
  onRemoved,
}: {
  person: PersonRow
  viewerRole: ViewerRole
  canReset: boolean
  refresh: () => Promise<void>
  writes: Writes
  onDialog: (kind: 'reset' | 'set') => void
  onRemoved: (username: string) => void
}): React.JSX.Element {
  const [draft, setDraft] = useState(person.fullName ?? '')
  const [unlocked, setUnlocked] = useState<string | null>(null)
  const saved = person.fullName ?? ''
  const dirty = draft.trim() !== saved
  const saveName = async (): Promise<void> => {
    if (await writes.run('name', () => daemonCliPost(primaryScope(), 'skin/users/full-name', { username: person.username, fullName: draft.trim() }))) await refresh()
  }
  const unlock = async (): Promise<void> => {
    let cleared = false
    const ok = await writes.run('unlock', async () => {
      const r = await daemonCliPost<unknown>(primaryScope(), 'skin/users/unlock', { username: person.username })
      cleared = Boolean(r && typeof r === 'object' && (r as Record<string, unknown>).cleared === true)
    })
    if (ok) {
      setUnlocked(cleared ? `${person.username} is unlocked and can sign in again.` : `${person.username} wasn’t locked.`)
      await refresh()
    }
  }
  const remove = async (): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Remove ${person.username}?`,
      message: `${person.username} is signed out everywhere and can’t sign in to this server or its Apps again. Their messages stay. To come back, they need to be added again.`,
      confirmLabel: `Remove ${person.username}`,
      destructive: true,
    })
    if (!ok) return
    if (await writes.run('remove', () => daemonCliPost(primaryScope(), 'skin/users/remove', { username: person.username }))) {
      onRemoved(person.username)
      await refresh()
    }
  }
  const lockedUntil = person.lockedUntil ? new Date(person.lockedUntil * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : null
  const passwordHint = !canReset && viewerRole === 'admin' ? BOX_LOGIN_REASON : undefined

  return (
    <>
      <SettingsGroup title="Profile" card testId="person-profile">
        <SettingRow
          label="Full name"
          hint="Shown to agents and in Apps in place of their username."
          below={<SaveError state={writes.at('name')} />}
        >
          {dirty ? (
            <Button variant="accent" aria-label={`Save ${person.username} full name`} disabled={writes.busy === 'name'} onClick={() => void saveName()}>
              {writes.busy === 'name' ? 'Saving…' : 'Save'}
            </Button>
          ) : (
            <SaveNote state={writes.at('name')} />
          )}
          <Input
            value={draft}
            aria-label={`${person.username} full name`}
            placeholder="No name yet"
            disabled={writes.busy === 'name'}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && dirty) void saveName()
            }}
            className={cx(SETTING_FIELD_WIDTH, 'type-control')}
          />
        </SettingRow>
        <SettingRow label="Username" hint="What they sign in with. It can’t be changed.">
          <span className={cx(SETTING_FIELD_WIDTH, 'type-row font-mono text-[var(--color-text-primary)] truncate')}>{person.username}</span>
        </SettingRow>
        <SettingRow label="Email" hint="Where K2 sends their invite and reset links.">
          <span className={cx(SETTING_FIELD_WIDTH, 'type-row text-[var(--color-text-primary)] truncate')}>{person.email ?? 'None'}</span>
        </SettingRow>
      </SettingsGroup>

      {viewerRole ? (
        <SettingsGroup title="Password" hint={passwordHint} card testId="person-password">
          {person.lockedUntil ? (
            <SettingRow
              label="Locked out"
              hint={`Too many wrong passwords. They can try again after ${lockedUntil}, or you can unlock them now.`}
              below={<SaveError state={writes.at('unlock')} />}
            >
              <Button variant="accent" disabled={!canReset || writes.busy === 'unlock'} onClick={() => void unlock()}>
                Unlock
              </Button>
            </SettingRow>
          ) : null}
          {unlocked ? (
            <SettingRow label="Unlocked" hint={unlocked}>
              <SaveNote state={writes.at('unlock')} />
            </SettingRow>
          ) : null}
          <SettingRow label="Reset password" hint="Send them a link to choose a new one, or get a one-time link to pass on.">
            <Button disabled={!canReset} onClick={() => onDialog('reset')}>
              Reset password…
            </Button>
          </SettingRow>
          <SettingRow label="Set a password" hint="Type or generate one and give it to them yourself. Signs them out everywhere.">
            <Button disabled={!canReset} onClick={() => onDialog('set')}>
              Set password…
            </Button>
          </SettingRow>
        </SettingsGroup>
      ) : null}

      <SettingsGroup title="Remove" card testId="person-remove">
        <SettingRow
          label={`Remove ${person.username}`}
          hint="Signs them out and takes away everything they can use here. Their messages stay."
          below={<SaveError state={writes.at('remove')} />}
        >
          <Button variant="danger-quiet" aria-label={`Remove ${person.username}…`} disabled={writes.busy === 'remove'} onClick={() => void remove()}>
            Remove…
          </Button>
        </SettingRow>
      </SettingsGroup>
    </>
  )
}
