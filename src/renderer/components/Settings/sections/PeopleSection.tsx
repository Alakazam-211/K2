// Settings → Access → User Access: the people who use this server's Apps
// (App guests), and which Apps each can use. A list and detail like
// Workspaces / Agents: the list on the left (Everyone, then each person),
// and on the right either everyone at a glance (by person, or by App) or one
// person, in two tabs (What they can use, Account).
//
// The page reads four parts on their own (user-access-load.ts): people,
// what each can use, roles and App names. One part failing or slow never
// blanks the rest; each says so where it shows, with its own Try again.
//
// The daemon's words (principals, grants) stay in user-access-api.ts; the
// page says "people" and "what they can use". Templates live under User
// Templates; logins for K2 itself under Admin Access (Connect users).

import React, { useCallback, useEffect, useState } from 'react'
import { daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import { primaryScope } from '@/kessel/server-scope'
import { Button, Input, OverflowMenu, Segmented, Surface, Tag, Toggle, cx } from '@/components/ui'
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
  SETTING_LIST_ROW_HOVER,
} from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { AccessAuditLink } from '../shared/AccessAuditLink'
import { SewPersonAccess } from './SewAccessParts'
import type { ChatGrant } from './sew-api'
import { AddUserDialog, AddedUserDialog, AppUserResetDialog, ResetLinkDialog, type AddedUser } from './AppUserDialogs'
import {
  HOST_APP,
  HOST_LABEL,
  parsePeople,
  reachWords,
  takePersonHandoff,
  type AppOption,
  type GrantRow,
  type PersonRow,
  type RoleRow,
} from './user-access-api'
import { useAccessData, type AppsData, type Part, type PartName } from './user-access-load'

export { parsePeople }
export type { PersonRow }

export const PEOPLE_MANIFEST: SettingEntry[] = [
  {
    id: 'people.roster',
    section: 'people',
    label: 'User Access',
    description: 'Who can use this server’s K2 page, its Apps and chat, and what each person reaches.',
    keywords: ['people', 'user access', 'full name', 'principal', 'guest', 'username', 'grant', 'password', 'app', 'unlock', 'reset password', 'role'],
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

type Selection = 'everyone' | string
type ViewerRole = 'owner' | 'admin' | null
type View = 'people' | 'apps'

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

/**
 * An App's name for the page: this server's K2 page, the App's name, or,
 * when the name isn't known, why: still loading (null), didn't load, or the
 * App is no longer published.
 */
function nameFor(id: string, apps: Part<AppsData>): string | null {
  if (id === HOST_APP) return HOST_LABEL
  const hit = apps.data?.apps.find((a) => a.id === id)
  if (hit) return hit.name
  if (apps.status === 'loading' && !apps.data) return null
  if (apps.status === 'error' || (apps.data?.missed ?? 0) > 0) return 'App name didn’t load'
  return 'An App that’s no longer published'
}

/** A grant's role, by name, or null when there's none (or roles didn't load). */
function roleName(grant: GrantRow, roles: RoleRow[]): string | null {
  return grant.roleId ? roles.find((r) => r.id === grant.roleId)?.name ?? null : null
}

/** The roles that fit an App: its own first, then the ones for any App. */
function rolesFor(app: string, roles: RoleRow[]): RoleRow[] {
  return [...roles.filter((r) => r.appId === app), ...roles.filter((r) => !r.appId)]
}

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

/** A person's App grants, this server's K2 page first. */
function appGrantsOf(person: PersonRow, grants: GrantRow[]): GrantRow[] {
  if (!person.id) return []
  const mine = grants.filter((g) => g.subjectKind === 'principal' && g.subjectId === person.id && g.kind === 'app')
  return [...mine.filter((g) => g.targetId === HOST_APP), ...mine.filter((g) => g.targetId !== HOST_APP)]
}

/** Each App a person can use, as a chip: "Shop · Staff"; an App turned off
 *  for them is quieter and says Off. */
function AccessChips({
  person,
  grants,
  roles,
  apps,
}: {
  person: PersonRow
  grants: Part<GrantRow[]>
  roles: RoleRow[]
  apps: Part<AppsData>
}): React.JSX.Element {
  if (!grants.data) {
    return (
      <span className="type-hint text-[var(--color-text-secondary)]" data-testid={`summary-${person.username}`}>
        {grants.status === 'error' ? 'Didn’t load' : 'Loading…'}
      </span>
    )
  }
  const mine = appGrantsOf(person, grants.data)
  const chat = person.id ? grants.data.find((g) => g.kind === 'chat' && g.subjectKind === 'principal' && g.subjectId === person.id && g.enabled) : undefined
  if (mine.length === 0 && !chat) {
    return (
      <span className="type-hint text-[var(--color-text-secondary)]" data-testid={`summary-${person.username}`}>
        No Apps yet
      </span>
    )
  }
  return (
    <span className="flex flex-wrap items-center gap-1.5 min-w-0" data-testid={`summary-${person.username}`}>
      {mine.map((g) => {
        const name = nameFor(g.targetId, apps) ?? 'Loading…'
        const role = roleName(g, roles)
        return (
          <Tag key={g.id} tone={g.enabled ? 'neutral' : 'muted'} data-case="as-written" data-app-chip={g.targetId}>
            {name}
            {role ? ` · ${role}` : ''}
            {g.enabled ? '' : ' · Off'}
          </Tag>
        )
      })}
      {chat ? (
        <Tag tone="neutral" data-case="as-written">
          {chat.scope === 'guest' ? 'Chat as a guest' : 'Chat'}
        </Tag>
      ) : null}
    </span>
  )
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

/** A part that failed while the rest of the page shows: what's missing, the
 *  server's reason, and Try again for that part alone. */
function PartFailed({
  part,
  title,
  children,
  onRetry,
}: {
  part: Part<unknown>
  title: string
  children: React.ReactNode
  onRetry: () => void
}): React.JSX.Element | null {
  if (part.status !== 'error') return null
  return (
    <SettingsLoadState kind="error" title={title} detail={part.error} testId="people-part-error" action={<Button onClick={onRetry}>Try again</Button>}>
      {children}
    </SettingsLoadState>
  )
}

export function PeopleSection(): React.JSX.Element {
  const [selection, setSelection] = useState<Selection>(() => takePersonHandoff() ?? 'everyone')
  const [view, setView] = useState<View>('people')
  const [needApps, setNeedApps] = useState(false)
  // 0.46.6: the daemon lets the Owner manage every App user's password
  // and an Admin those of people who only use Apps (never a box login);
  // adding a person stays the Owner's. Nobody sees actions that would fail.
  const [viewerRole, setViewerRole] = useState<ViewerRole>(null)
  const isOwner = viewerRole === 'owner'
  const [dialog, setDialog] = useState<{ kind: 'reset' | 'link'; username: string } | null>(null)
  const [adding, setAdding] = useState(false)
  // The one-time result of Add person; dropped when its sheet closes.
  const [added, setAdded] = useState<AddedUser | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  const { users, grants, roles, apps, reload, refresh } = useAccessData(needApps || view === 'apps' || adding)

  useEffect(() => {
    let live = true
    void fetchViewerRole().then((role) => {
      if (live) setViewerRole(role === 'owner' || role === 'admin' ? role : null)
    })
    return () => {
      live = false
    }
  }, [])

  const loadApps = useCallback(async (): Promise<void> => {
    setNeedApps(true)
    if (apps.status === 'error') await reload('apps')
  }, [apps.status, reload])

  const people = users.data ?? []
  const roleRows = roles.data ?? []
  const person = selection !== 'everyone' ? people.find((p) => p.username === selection) ?? null : null
  const dialogPerson = dialog ? people.find((p) => p.username === dialog.username) ?? null : null
  const postHere = (route: string, body: Record<string, unknown>): Promise<unknown> => daemonCliPost(primaryScope(), route, body)
  const select = (next: Selection): void => {
    setSelection(next)
    setNotice(null)
  }
  const openDialog = (kind: 'reset' | 'link', username: string): void => setDialog({ kind, username })

  const ready = users.data != null
  const roleChoices = roleRows.map((r) => ({ value: r.id, label: r.name, appId: r.appId }))
  const appChoices = (apps.data?.apps ?? []).map((a) => ({ value: a.id, label: `${a.name} (${a.kind === 'skin' ? 'app' : a.kind})` }))

  return (
    <div className="h-full min-h-0 flex">
      <aside aria-label="People" className="w-72 flex-shrink-0 min-h-0 flex flex-col border-r border-[var(--color-border)]">
        <div className="flex-shrink-0 px-4 pt-6 pb-3">
          <h2 className="flex items-center h-7 type-heading text-[var(--color-text-primary)]">User Access</h2>
          <p className="type-hint text-[var(--color-text-secondary)] leading-relaxed">
            The people who use your Apps, and what each one can use.
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
          {!ready && users.status !== 'error' ? (
            <>
              <EveryoneHeader isOwner={false} onAdd={() => setAdding(true)} waiting />
              <SettingsLoadState
                kind="loading"
                title={users.waiting ? 'Waiting for the server to reconnect…' : 'Loading people…'}
                testId="people-loading"
              >
                {users.waiting ? 'The list loads by itself once the server is back.' : 'Reading who can use your Apps.'}
              </SettingsLoadState>
            </>
          ) : !ready ? (
            <>
              <EveryoneHeader isOwner={false} onAdd={() => setAdding(true)} waiting />
              <SettingsLoadState
                kind="error"
                title="Couldn’t load people"
                detail={users.error}
                testId="people-error"
                action={<Button onClick={() => void refresh()}>Try again</Button>}
              >
                K2 couldn’t read who can use your Apps. Nothing has changed.
              </SettingsLoadState>
            </>
          ) : person ? (
            <PersonDetail
              key={person.username}
              person={person}
              grants={grants}
              roles={roles}
              apps={apps}
              viewerRole={viewerRole}
              notice={notice}
              loadApps={loadApps}
              reload={reload}
              refresh={refresh}
              onDialog={(kind) => openDialog(kind, person.username)}
              onRemoved={(username) => {
                setSelection('everyone')
                setNotice(`${username} was removed. They can no longer sign in here.`)
              }}
            />
          ) : (
            <Everyone
              people={people}
              users={users}
              grants={grants}
              roles={roles}
              apps={apps}
              view={view}
              onView={setView}
              viewerRole={viewerRole}
              notice={notice}
              onAdd={() => setAdding(true)}
              onOpen={select}
              onDialog={openDialog}
              reload={reload}
            />
          )}
        </div>
      </div>

      {dialogPerson && dialog?.kind === 'reset' ? (
        <AppUserResetDialog
          key={`reset:${dialogPerson.username}`}
          person={dialogPerson}
          post={postHere}
          onDone={() => {
            setNotice(`${dialogPerson.username}’s password was reset. Send them the new one if you haven’t yet.`)
            void refresh()
          }}
          onClose={() => setDialog(null)}
          onLink={() => setDialog({ kind: 'link', username: dialogPerson.username })}
        />
      ) : null}
      {dialogPerson && dialog?.kind === 'link' ? (
        <ResetLinkDialog
          key={`link:${dialogPerson.username}`}
          person={dialogPerson}
          post={postHere}
          onClose={() => {
            setDialog(null)
            void refresh()
          }}
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
              selected ? 'bg-[var(--color-accent)]/15' : SETTING_LIST_ROW_HOVER,
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
          {waiting ? 'Which Apps each person can use.' : 'Which Apps each person can use. Pick someone to change it or reset their password.'}
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

/** Everyone at a glance, by person (each with their Apps as chips) or by
 *  App (each App with its people); or how people get here when there's no
 *  one yet. Parts that failed or are still loading say so in place. */
function Everyone({
  people,
  users,
  grants,
  roles,
  apps,
  view,
  onView,
  viewerRole,
  notice,
  onAdd,
  onOpen,
  onDialog,
  reload,
}: {
  people: PersonRow[]
  users: Part<PersonRow[]>
  grants: Part<GrantRow[]>
  roles: Part<RoleRow[]>
  apps: Part<AppsData>
  view: View
  onView: (v: View) => void
  viewerRole: ViewerRole
  notice: string | null
  onAdd: () => void
  onOpen: (username: string) => void
  onDialog: (kind: 'reset' | 'link', username: string) => void
  reload: (part: PartName) => Promise<void>
}): React.JSX.Element {
  const isOwner = viewerRole === 'owner'
  if (people.length === 0) {
    return (
      <>
        <EveryoneHeader isOwner={false} onAdd={onAdd} waiting />
        <SettingsLoadState
          kind="empty"
          title="No one uses your Apps yet"
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
  const roleRows = roles.data ?? []
  return (
    <>
      <EveryoneHeader isOwner={isOwner} onAdd={onAdd} />
      {notice ? (
        <p role="status" className="type-hint text-[var(--color-status-ok-soft)]">
          {notice}
        </p>
      ) : null}
      <PartFailed part={users} title="Couldn’t refresh the list" onRetry={() => void reload('users')}>
        This is the list as it was. Changes since then may be missing.
      </PartFailed>
      <PartFailed part={grants} title="Couldn’t load which Apps each person can use" onRetry={() => void reload('grants')}>
        The people are listed; their Apps show once this loads.
      </PartFailed>
      <PartFailed part={roles} title="Couldn’t load the roles" onRetry={() => void reload('roles')}>
        Each person’s Apps are shown without their role until this loads.
      </PartFailed>
      <PartFailed part={apps} title="Couldn’t load the App names" onRetry={() => void reload('apps')}>
        Apps are shown without their names until this loads.
      </PartFailed>
      {apps.status === 'ready' && (apps.data?.missed ?? 0) > 0 ? (
        <SettingsLoadState
          kind="error"
          title="Some App names didn’t load"
          detail={apps.data?.reason}
          testId="people-apps-missed"
          action={<Button onClick={() => void reload('apps')}>Try again</Button>}
        >
          {apps.data?.missed === 1 ? 'One workspace didn’t answer' : `${apps.data?.missed} workspaces didn’t answer`}, so
          its Apps show without a name. Everything else is listed.
        </SettingsLoadState>
      ) : null}
      <section data-testid="people-overview">
        <div className="mb-2 flex items-end justify-between gap-4">
          <div className="min-w-0">
            <h3 className="flex items-center min-h-[1.5rem] type-heading text-[var(--color-text-primary)]">
              {view === 'people' ? 'People' : 'Apps'}
            </h3>
            <p className={SETTING_ROW_HINT}>
              {view === 'people' ? 'Each person and the Apps they can use.' : 'Each App and the people who can use it.'}
            </p>
          </div>
          <Segmented<View>
            aria-label="Show by"
            value={view}
            onChange={onView}
            options={[
              { value: 'people', label: 'By person' },
              { value: 'apps', label: 'By App' },
            ]}
          />
        </div>
        {view === 'people' ? (
          // overflow-hidden: a hovered row keeps to the card's corners.
          <Surface chamfer material="tile" className="overflow-hidden divide-y divide-[var(--color-border)]">
            {people.map((row) => (
              <EveryoneRow
                key={row.username}
                person={row}
                grants={grants}
                roles={roleRows}
                apps={apps}
                viewerRole={viewerRole}
                onOpen={onOpen}
                onDialog={onDialog}
              />
            ))}
          </Surface>
        ) : (
          <ByApp people={people} grants={grants} roles={roleRows} apps={apps} onOpen={onOpen} />
        )}
      </section>
    </>
  )
}

/** One person in the list of everyone: name, their Apps, and a menu with
 *  Reset password. The row opens the person; the menu sits beside it. */
function EveryoneRow({
  person,
  grants,
  roles,
  apps,
  viewerRole,
  onOpen,
  onDialog,
}: {
  person: PersonRow
  grants: Part<GrantRow[]>
  roles: RoleRow[]
  apps: Part<AppsData>
  viewerRole: ViewerRole
  onOpen: (username: string) => void
  onDialog: (kind: 'reset' | 'link', username: string) => void
}): React.JSX.Element {
  const canReset = canResetPassword(viewerRole, person)
  const why = !canReset && viewerRole === 'admin' ? BOX_LOGIN_REASON : undefined
  return (
    <div className={`relative flex items-center gap-2 pr-3 ${SETTING_LIST_ROW_HOVER}`} data-overview-item={person.username}>
      <button
        type="button"
        aria-label={`See what ${person.username} can use`}
        data-overview-row={person.username}
        data-k2-focus=""
        onClick={() => onOpen(person.username)}
        className="relative min-w-0 flex-1 flex items-center gap-4 pl-4 pr-1 py-3 text-left no-drag cursor-pointer outline-none"
      >
        <span className="min-w-0 w-52 flex-shrink-0">
          <span className="block type-name text-[var(--color-text-primary)] truncate">{displayName(person)}</span>
          <span className="flex items-center gap-2 min-w-0">
            <span className="type-hint text-[var(--color-text-secondary)] truncate">{secondLine(person)}</span>
            <PersonTags person={person} />
          </span>
        </span>
        <span className="min-w-0 flex-1">
          <AccessChips person={person} grants={grants} roles={roles} apps={apps} />
        </span>
        <Chevron />
      </button>
      {viewerRole ? (
        <OverflowMenu
          label={`More actions for ${person.username}`}
          items={[
            { id: 'open', label: 'See what they can use', onSelect: () => onOpen(person.username) },
            { id: 'reset', label: 'Reset password…', group: true, disabled: !canReset, hint: why, onSelect: () => onDialog('reset', person.username) },
            { id: 'link', label: 'Send a reset link…', disabled: !canReset, onSelect: () => onDialog('link', person.username) },
          ]}
        />
      ) : null}
    </div>
  )
}

/** Each App and the people who can use it: this server's K2 page, then each
 *  published App, then any App someone has that's no longer listed. */
function ByApp({
  people,
  grants,
  roles,
  apps,
  onOpen,
}: {
  people: PersonRow[]
  grants: Part<GrantRow[]>
  roles: RoleRow[]
  apps: Part<AppsData>
  onOpen: (username: string) => void
}): React.JSX.Element {
  if (!grants.data) {
    return grants.status === 'error' ? (
      <SettingsLoadState kind="error" title="Nothing to group yet" testId="people-by-app-error">
        Which Apps each person can use didn’t load. Try again above.
      </SettingsLoadState>
    ) : (
      <SettingsLoadState kind="loading" title="Loading Apps…" testId="people-by-app-loading">
        Reading which Apps each person can use.
      </SettingsLoadState>
    )
  }
  const appGrants = grants.data.filter((g) => g.kind === 'app' && g.subjectKind === 'principal')
  const ids = [HOST_APP, ...(apps.data?.apps ?? []).map((a) => a.id)]
  for (const g of appGrants) if (!ids.includes(g.targetId)) ids.push(g.targetId)
  return (
    <div className="space-y-4" data-testid="people-by-app">
      {apps.status === 'loading' && !apps.data ? (
        <SettingsLoadState kind="loading" title="Loading the list of Apps…" testId="people-apps-loading">
          Apps that people already use are shown below.
        </SettingsLoadState>
      ) : null}
      {ids.map((id) => {
        const rows = appGrants.filter((g) => g.targetId === id)
        const name = nameFor(id, apps) ?? 'Loading…'
        return (
          <SettingsGroup
            key={id}
            title={name}
            hint={rows.length === 0 ? undefined : `${rows.length} ${rows.length === 1 ? 'person' : 'people'}`}
            card
            testId={`by-app-${id}`}
          >
            {rows.length === 0 ? (
              <SettingRow label="No one can use this App yet" hint="Pick a person and give them access to it.">
                {null}
              </SettingRow>
            ) : (
              rows.map((g) => {
                const who = people.find((p) => p.id === g.subjectId)
                if (!who) return null
                const role = roleName(g, roles)
                return (
                  <button
                    key={g.id}
                    type="button"
                    aria-label={`See what ${who.username} can use`}
                    data-k2-focus=""
                    onClick={() => onOpen(who.username)}
                    className={`relative w-full flex items-center gap-4 px-4 py-3 text-left no-drag cursor-pointer outline-none ${SETTING_LIST_ROW_HOVER}`}
                  >
                    <span className="min-w-0 flex-1">
                      <span className="block type-name text-[var(--color-text-primary)] truncate">{displayName(who)}</span>
                      <span className="block type-hint text-[var(--color-text-secondary)] truncate">{secondLine(who)}</span>
                    </span>
                    <span className="type-row text-[var(--color-text-secondary)]">{role ?? 'No role'}</span>
                    {g.enabled ? null : <Tag tone="muted">Off</Tag>}
                    <Chevron />
                  </button>
                )
              })
            )}
          </SettingsGroup>
        )
      })}
    </div>
  )
}

type Tab = 'access' | 'account'
const TAB_LABEL: Record<Tab, string> = { access: 'What they can use', account: 'Account' }

function PersonDetail({
  person,
  grants,
  roles,
  apps,
  viewerRole,
  notice,
  loadApps,
  reload,
  refresh,
  onDialog,
  onRemoved,
}: {
  person: PersonRow
  grants: Part<GrantRow[]>
  roles: Part<RoleRow[]>
  apps: Part<AppsData>
  viewerRole: ViewerRole
  notice: string | null
  loadApps: () => Promise<void>
  reload: (part: PartName) => Promise<void>
  refresh: () => Promise<void>
  onDialog: (kind: 'reset' | 'link') => void
  onRemoved: (username: string) => void
}): React.JSX.Element {
  const [tab, setTab] = useState<Tab>('access')
  const writes = useRowWrites()
  const canReset = canResetPassword(viewerRole, person)
  const mine = person.id && grants.data ? grants.data.filter((g) => g.subjectKind === 'principal' && g.subjectId === person.id) : []
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
          {viewerRole ? (
            <Button
              className="flex-shrink-0"
              disabled={!canReset}
              title={canReset ? undefined : BOX_LOGIN_REASON}
              onClick={() => onDialog('reset')}
            >
              Reset password…
            </Button>
          ) : null}
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
      {notice ? (
        <p role="status" data-testid="person-notice" className="type-hint text-[var(--color-status-ok-soft)]">
          {notice}
        </p>
      ) : null}

      {tab === 'access' ? (
        <>
          {person.tiedTo ? <ResetPanel person={person} onReset={canReset ? () => onDialog('link') : null} /> : null}
          <SignInCard
            person={person}
            grants={grants}
            mine={mine}
            roles={roles}
            apps={apps}
            loadApps={loadApps}
            reload={reload}
            refresh={refresh}
            writes={writes}
          />
          {grants.data ? <SewPersonAccess principalId={person.id} username={person.username} grant={sewGrant} onChanged={refresh} /> : null}
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
 * own password, through the App or the owner's reset link (a one-time link,
 * or mailed to an address you give). The stored email is never used for the
 * owner's reset: an agent could have chosen it. Nothing for an untied person.
 * `onReset` opens Send a reset link (the owner, or an Admin for someone who
 * only uses Apps; null hides it).
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
        {onReset ? <Button onClick={onReset}>Send a reset link…</Button> : null}
      </SettingRow>
    </SettingsGroup>
  )
}

type Writes = ReturnType<typeof useRowWrites>

/** Where they can sign in: this server's K2 page, then each App, each with
 *  its role (changed in place, saved at once), on/off, and Remove. */
function SignInCard({
  person,
  grants,
  mine,
  roles,
  apps,
  loadApps,
  reload,
  refresh,
  writes,
}: {
  person: PersonRow
  grants: Part<GrantRow[]>
  mine: GrantRow[]
  roles: Part<RoleRow[]>
  apps: Part<AppsData>
  loadApps: () => Promise<void>
  reload: (part: PartName) => Promise<void>
  refresh: () => Promise<void>
  writes: Writes
}): React.JSX.Element {
  const [adding, setAdding] = useState(false)
  const [pickApp, setPickApp] = useState('')
  const [pickRole, setPickRole] = useState('')
  const [notice, setNotice] = useState<string | null>(null)
  const roleRows = roles.data ?? []
  const host = mine.find((g) => g.kind === 'app' && g.targetId === HOST_APP)
  const appGrants = mine.filter((g) => g.kind === 'app' && g.targetId !== HOST_APP)
  const roleOf = (id: string | null): RoleRow | null => (id ? roleRows.find((r) => r.id === id) ?? null : null)
  const hostRole = roleOf(host?.roleId ?? person.roleId)
  const first = person.fullName?.split(' ')[0] ?? person.username
  const name = (id: string): string => nameFor(id, apps) ?? 'Loading…'
  const said = (text: string): void => setNotice(text)

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
    setNotice(null)
    if (await writes.run('host', () => daemonCliPost(primaryScope(), 'skin/grants/host', { username: person.username, present: on }))) await refresh()
  }
  const setEnabled = async (grant: GrantRow, on: boolean): Promise<void> => {
    setNotice(null)
    if (await writes.run(grant.id, () => daemonCliPost(primaryScope(), 'skin/grants/enabled', { id: grant.id, enabled: on }))) await refresh()
  }
  const setRole = async (grant: GrantRow, role: string): Promise<void> => {
    if (role === grant.roleId) return
    setNotice(null)
    if (await writes.run(grant.id, () => daemonCliPost(primaryScope(), 'skin/grants/role', { id: grant.id, role }))) await refresh()
  }
  const remove = async (grant: GrantRow): Promise<void> => {
    const app = name(grant.targetId)
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Remove ${person.username}’s access to ${app}?`,
      message: `${person.username} is signed out of ${app} and loses the role you gave them there. You can give it back later.`,
      confirmLabel: 'Remove access',
      destructive: true,
    })
    if (!ok) return
    setNotice(null)
    if (await writes.run('apps', () => daemonCliPost(primaryScope(), 'skin/grants/delete', { id: grant.id }))) {
      said(`${person.username} can no longer sign in to ${app}.`)
      await refresh()
    }
  }
  const add = async (): Promise<void> => {
    if (!person.id || !pickApp || !pickRole) return
    const app = name(pickApp)
    const role = roleOf(pickRole)?.name
    setNotice(null)
    const done = await writes.run('apps', () =>
      daemonCliPost(primaryScope(), 'skin/grants', { subjectKind: 'principal', subjectId: person.id, kind: 'app', targetId: pickApp, roleId: pickRole }),
    )
    if (done) {
      setAdding(false)
      setPickApp('')
      setPickRole('')
      said(`${person.username} can now sign in to ${app}${role ? ` as ${role}` : ''}.`)
      await refresh()
    }
  }

  const openForm = (): void => {
    setAdding(true)
    setNotice(null)
    void loadApps()
  }
  const appList: AppOption[] = apps.data?.apps ?? []
  const free = appList.filter((a) => !appGrants.some((g) => g.targetId === a.id))
  const pickRoles = pickApp ? rolesFor(pickApp, roleRows) : []
  const roleOptions = (app: string): { value: string; label: string }[] => rolesFor(app, roleRows).map((r) => ({ value: r.id, label: r.name }))

  /** The role picker on a row: saves as soon as it's changed. */
  const rolePicker = (grant: GrantRow, label: string): React.ReactNode => (
    <SettingDropdown
      ariaLabel={`Role in ${label} for ${person.username}`}
      value={grant.roleId ?? ''}
      placeholder={roles.status === 'error' ? 'Roles didn’t load' : roles.data ? 'No role' : 'Loading…'}
      disabled={!roles.data || writes.busy === grant.id}
      options={roleOptions(grant.targetId)}
      onChange={(v) => void setRole(grant, v)}
    />
  )

  const body = !grants.data ? (
    grants.status === 'error' ? (
      <SettingsLoadState
        kind="error"
        title={`Couldn’t load what ${first} can use`}
        detail={grants.error}
        testId="person-grants-error"
        action={<Button onClick={() => void reload('grants')}>Try again</Button>}
      >
        Nothing has changed.
      </SettingsLoadState>
    ) : (
      <SettingsLoadState kind="loading" title={`Loading what ${first} can use…`} testId="person-grants-loading" />
    )
  ) : null

  return (
    <SettingsGroup
      title="Sign-in and Apps"
      hint={`Where ${first} can sign in, and the role that sets which agents they reach there.`}
      card
      testId="person-sign-in"
      state={body}
      action={
        <span className="flex items-center gap-2">
          <SaveNote state={writes.at('apps')} />
          {!adding ? (
            <Button onClick={openForm} disabled={!person.id || !grants.data}>
              Give access to an App
            </Button>
          ) : null}
        </span>
      }
    >
      {notice ? (
        <p role="status" data-testid="person-sign-in-notice" className="px-4 py-2.5 type-hint text-[var(--color-status-ok-soft)]">
          {notice}
        </p>
      ) : null}
      {roles.status === 'error' ? (
        <SettingRow
          label="Roles didn’t load"
          hint={`Each App is listed, but its role can’t be shown or changed until they load. The server said: ${(roles.error ?? '').replace(/[.\s]+$/, '')}.`}
        >
          <Button onClick={() => void reload('roles')}>Try again</Button>
        </SettingRow>
      ) : null}
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
        const label = name(grant.targetId)
        return (
          <SettingRow
            key={grant.id}
            label={label}
            hint={grant.enabled ? reachWords(roleOf(grant.roleId), [], false) : 'Off. They can’t sign in to it until you turn it back on.'}
            below={<SaveError state={writes.at(grant.id)} />}
            testId={`person-app-${grant.targetId}`}
          >
            <SaveNote state={writes.at(grant.id)} />
            {rolePicker(grant, label)}
            {/* Before the switch, so every switch in the card shares one edge. */}
            <OverflowMenu
              label={`More actions for ${label}`}
              items={[{ id: 'remove', label: 'Remove access…', danger: true, onSelect: () => void remove(grant) }]}
            />
            <Toggle
              checked={grant.enabled}
              aria-label={`${label} for ${person.username}`}
              disabled={writes.busy === grant.id}
              onChange={(on) => void setEnabled(grant, on)}
            />
          </SettingRow>
        )
      })}
      {appGrants.length === 0 && !adding ? (
        <SettingRow label="No Apps yet" hint={`Give ${first} access to an App to let them sign in to it.`}>
          {null}
        </SettingRow>
      ) : null}
      {adding ? (
        <div data-testid="person-add-app">
          <SettingRow
            label="App"
            hint={apps.status === 'error' ? undefined : apps.data && free.length === 0 ? `${first} can already use every App on this server.` : 'The Apps published on this server.'}
            below={
              apps.status === 'error' ? (
                <p role="alert" className="type-hint text-[var(--color-status-error-soft)]">
                  Couldn’t list the Apps: {(apps.error ?? '').replace(/[.\s]+$/, '')}.{' '}
                  <button type="button" data-k2-focus="" className="relative px-0.5 -mx-0.5 underline cursor-pointer" onClick={() => void reload('apps')}>
                    Try again
                  </button>
                </p>
              ) : undefined
            }
            field
          >
            <SettingDropdown
              ariaLabel="App to add"
              value={pickApp}
              placeholder={apps.data ? 'Choose an App' : 'Loading Apps…'}
              disabled={!apps.data || free.length === 0}
              fullWidth
              options={free.map((a) => ({ value: a.id, label: a.name }))}
              onChange={(v) => {
                setPickApp(v)
                // The App's own role first; any role still fits.
                setPickRole(rolesFor(v, roleRows)[0]?.id ?? '')
              }}
            />
          </SettingRow>
          <SettingRow
            label="Role"
            hint={roleRows.length ? 'Sets which agents they reach in this App.' : 'No roles yet. Make one in Settings → Apps first.'}
            field
          >
            <SettingDropdown
              ariaLabel="Role in the App"
              value={pickRole}
              placeholder={pickApp ? 'Choose a role' : 'Choose an App first'}
              disabled={pickRoles.length === 0}
              fullWidth
              options={pickRoles.map((r) => ({ value: r.id, label: r.name }))}
              onChange={setPickRole}
            />
          </SettingRow>
          <div className="px-4 py-3 flex items-center justify-end gap-2">
            <Button variant="quiet" onClick={() => setAdding(false)}>
              Cancel
            </Button>
            <Button variant="accent" disabled={!pickApp || !pickRole || writes.busy === 'apps'} onClick={() => void add()}>
              {writes.busy === 'apps' ? 'Giving access…' : 'Give access'}
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
  onDialog: (kind: 'reset' | 'link') => void
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
          <SettingRow label="Reset password" hint="Make a new password, or type one, and send it to them. Signs them out everywhere.">
            <Button disabled={!canReset} onClick={() => onDialog('reset')}>
              Reset password…
            </Button>
          </SettingRow>
          <SettingRow
            label="Send a reset link"
            hint="They choose their own password from a one-time link. This also lifts an agent’s hold on them."
          >
            <Button disabled={!canReset} onClick={() => onDialog('link')}>
              Send a reset link…
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
