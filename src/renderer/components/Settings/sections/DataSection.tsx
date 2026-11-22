// Settings → Database (prd-workspace-data-sidecar-v1 D23): the owner's page
// for the Postgres server K2 runs on this box, the workspace databases on
// it, and who else may use each one (sql_grants: read / write / manage).
// Thin client: the daemon's `/cli/db/*` is the only catalog. Grants are
// workspaces, never App people. No fetchProjects in render paths (pickers
// read the already-loaded store; grant changes are optimistic).
//
// Layout: the Settings reference pattern. A page header, a tab strip
// (Databases / Server), card groups of rows. Databases is a list that
// opens one database (its access and agent sign-in) with a way back.
// Destructive actions are quiet buttons that confirm in the shared dialog
// frame (AccessDialogs); a dialog's failure stays in the dialog, and a
// page-level result is a dismissable note under the header.
//
// CAPABILITY: the page keys off GET /cli/db/status `supported` (the
// daemon's report, never the platform). Unsupported: sample data under
// the shared Linux note, every change off, no further network calls.

import React, { useCallback, useEffect, useId, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useWindowModeStore } from '@/stores/window-mode'
import { Button, Callout, Input, Toggle } from '@/components/ui'
import {
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SETTING_SERVER_REASON,
  SaveError,
  SaveNote,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  useSaveStatus,
} from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { AccessSheet, DIALOG_LABEL, DIALOG_MUTED, FailedNote } from './AccessDialogs'
import { HealthWord, StatusDot, type HealthTone } from './ConnectionsParts'
import { LinuxOnlyNote } from './LinuxOnlyNote'
import {
  SAMPLE_DATABASES,
  SAMPLE_STATUS,
  bindSqlRole,
  createSqlDatabase,
  dbVersionWord,
  disableSqlServer,
  enableSqlServer,
  fetchSqlDatabases,
  fetchSqlStatus,
  formatSqlListen,
  grantSqlAccess,
  revokeSqlAccess,
  sqlErrorMessage,
  sqlNotBaked,
  sqlRefused,
  sqlStateWord,
  abortLiveMove,
  fetchLiveStatus,
  livePlainLine,
  liveStateWord,
  prepareLiveMoves,
  switchLiveMove,
  type LiveStatus,
  type SqlDatabase,
  type SqlLevel,
  type SqlParticipant,
  type SqlStatus,
} from './data-api'
import { PRODUCT_NAME } from '@/lib/brand'

export const DATA_MANIFEST: SettingEntry[] = [
  {
    id: 'data.server',
    section: 'data',
    label: 'Database server',
    description: 'Turn the Postgres server on this server on or off, and see its address',
    keywords: ['sql', 'postgres', 'database', 'sidecar', 'db', 'enable', 'server'],
  },
  {
    id: 'data.databases',
    section: 'data',
    label: 'Databases',
    description: 'One database per workspace, for its agents and apps',
    keywords: ['database', 'sql', 'jsonb', 'documents', 'workspace', 'cap', 'create'],
  },
  {
    id: 'data.grants',
    section: 'data',
    label: 'Database access',
    description: 'Let other workspaces read, change or manage a database',
    keywords: ['grant', 'access', 'read', 'write', 'manage', 'workspace'],
  },
  {
    id: 'data.live',
    section: 'data',
    label: 'Move to a newer Postgres',
    description: 'Move a database to a newer Postgres while it keeps working: switch over or stop the move',
    keywords: ['live', 'dual-run', 'logical replication', 'upgrade', 'postgres', 'switch', 'abort', 'wal_level'],
  },
  {
    id: 'data.bind',
    section: 'data',
    label: 'Agent sign-in role',
    description: 'The Postgres role a workspace’s agents sign in as',
    keywords: ['bind', 'role', 'pg_role', 'rls', 'agent'],
  },
]

type Tab = 'databases' | 'server'
const TAB_LABEL: Record<Tab, string> = { databases: 'Databases', server: 'Server' }
const TAB_SETTINGS_ID: Record<Tab, string> = { databases: 'data.databases', server: 'data.server' }

/** A page-level result: what happened, and (for a failure) the server's reason. */
type Notice = { tone: 'ok' | 'error'; text: string; detail?: string }

const LEVEL_OPTIONS = [
  { value: 'read', label: 'Read' },
  { value: 'write', label: 'Read and change' },
]

function fmtDay(unixSecs: number): string | null {
  if (!unixSecs || unixSecs <= 0) return null
  return new Date(unixSecs * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' })
}

function participantName(p: Pick<SqlParticipant, 'workspace' | 'projectId'>, names: Map<string, string>): string {
  return p.workspace ?? names.get(p.projectId) ?? 'A removed workspace'
}

/** A status as a dot and a word, as on Email Hosting: the word carries it,
 *  colour only on a problem. */
function StatusWord({ tone, children }: { tone: HealthTone; children: React.ReactNode }): React.JSX.Element {
  return (
    <span className="inline-flex items-center gap-1.5 type-hint whitespace-nowrap" data-status-word={tone}>
      <StatusDot tone={tone} />
      <HealthWord tone={tone}>{children}</HealthWord>
    </span>
  )
}

/** A database's catalog status in a word. */
function dbStatusWord(status: string): { word: string; tone: HealthTone } {
  switch (status) {
    case 'active':
      return { word: 'Ready', tone: 'ok' }
    case 'test':
      return { word: 'Test copy', tone: 'neutral' }
    case 'creating':
    case 'restoring':
      return { word: status === 'creating' ? 'Creating' : 'Restoring', tone: 'warn' }
    default:
      return { word: status ? status.charAt(0).toUpperCase() + status.slice(1) : 'Unknown', tone: 'neutral' }
  }
}

/** A short value (an address) with Copy beside it. */
function CopyValue({ value, label }: { value: string; label: string }): React.JSX.Element {
  const [copy, setCopy] = useState<'idle' | 'done' | 'failed'>('idle')
  const doCopy = (): void => {
    const clip = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
    if (!clip) return setCopy('failed')
    clip.writeText(value).then(
      () => setCopy('done'),
      () => setCopy('failed'),
    )
  }
  return (
    <span className="flex items-center gap-2">
      <span className="type-hint font-mono text-[var(--color-text-secondary)] select-all" data-testid="db-listen">
        {value}
      </span>
      <Button aria-label={`Copy the ${label}`} onClick={doCopy}>
        {copy === 'done' ? 'Copied' : copy === 'failed' ? 'Couldn’t copy' : 'Copy'}
      </Button>
    </span>
  )
}

/** A confirm-first action in the shared dialog frame. Cancel has the focus;
 *  the action runs in place, and a failure stays in the dialog with the
 *  server's reason so the person can try again or cancel. A permission
 *  refusal says who can do it instead and offers no retry. */
function ActionSheet({
  title,
  message,
  confirmLabel,
  busyLabel,
  destructive = false,
  failWhat,
  failNext,
  refusedWhat,
  run,
  onDone,
  onClose,
  testId,
}: {
  title: string
  message: React.ReactNode
  confirmLabel: string
  busyLabel: string
  destructive?: boolean
  failWhat: string
  failNext: string
  /** What to say when the server refuses on permission ("Only …"). */
  refusedWhat: string
  run: () => Promise<unknown>
  onDone: () => void
  onClose: () => void
  testId: string
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [refused, setRefused] = useState(false)
  const go = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await run()
      onDone()
    } catch (e) {
      setRefused(sqlRefused(e))
      setError(sqlErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <AccessSheet
      title={title}
      onClose={onClose}
      busy={busy}
      testId={testId}
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus>
            {refused ? 'Close' : 'Cancel'}
          </Button>
          {refused ? null : (
            <Button variant={destructive ? 'danger-muted' : 'accent'} size="md" onClick={() => void go()} disabled={busy} aria-busy={busy || undefined}>
              {busy ? busyLabel : error ? 'Try again' : confirmLabel}
            </Button>
          )}
        </>
      }
    >
      <p className={DIALOG_MUTED}>{message}</p>
      {error ? <FailedNote what={refused ? refusedWhat : failWhat} detail={error} next={refused ? 'Nothing changed.' : failNext} /> : null}
    </AccessSheet>
  )
}

// ── Server tab ────────────────────────────────────────────────────────────

function statusSentence(status: SqlStatus): string {
  const pg = status.installedMajor != null ? `Postgres ${status.installedMajor}` : 'Postgres'
  switch (status.state) {
    case 'running':
      return `${pg} is running. Agents and apps on this server can use their databases.`
    case 'degraded':
      return `${pg} is running, but ${PRODUCT_NAME} found a problem. Databases may be slow or miss changes.`
    case 'installing':
      return `${pg} is starting. Databases are ready in a moment.`
    case 'error':
      return `${pg} stopped because of an error. Databases can’t be used until it runs again.`
    default:
      return `${pg} isn’t running.`
  }
}

function LiveMovesGroup({
  canMutate,
  setNotice,
}: {
  canMutate: boolean
  setNotice: (n: Notice | null) => void
}): React.JSX.Element | null {
  const [live, setLive] = useState<LiveStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [ask, setAsk] = useState<
    | { kind: 'switch'; db: string; to: number; from: number }
    | { kind: 'abort'; db: string; from: number; to: number }
    | { kind: 'prepare'; major: number }
    | null
  >(null)

  const load = useCallback(async (): Promise<void> => {
    try {
      setLive(await fetchLiveStatus())
      setError(null)
    } catch (e) {
      setError(sqlErrorMessage(e))
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  if (error) {
    return (
      <SettingsGroup
        card
        title="Move to a newer Postgres"
        settingId="data.live"
        testId="data-live-moves"
        state={
          <SettingsLoadState kind="error" title="Couldn’t check for moves to a newer Postgres" detail={error} action={<Button onClick={() => void load()}>Try again</Button>}>
            Your databases keep working. Try again in a moment.
          </SettingsLoadState>
        }
      />
    )
  }
  if (!live || (live.runs.length === 0 && live.notLogical.length === 0)) return null

  return (
    <>
      <SettingsGroup
        card
        title="Move to a newer Postgres"
        hint="A database can move to a newer Postgres version while it keeps working. When the copy is ready, switch over."
        settingId="data.live"
        testId="data-live-moves"
      >
        {live.runs.map((r) => {
          const word = liveStateWord(r.state)
          const tone = r.state === 'stalled' ? 'warn' : r.state === 'streaming' ? 'ok' : 'neutral'
          return (
            <SettingRow
              key={r.db}
              testId={`data-live-${r.db}`}
              label={
                <span>
                  <span className="font-mono">{r.db}</span>: Postgres {r.from} to {r.to}
                </span>
              }
              hint={
                <>
                  <StatusWord tone={tone}>{word}</StatusWord>{' '}
                  {livePlainLine(r)}
                  {r.lastError ? <span className={`block ${SETTING_SERVER_REASON}`}>The server said: {r.lastError.replace(/[.\s]+$/, '')}.</span> : null}
                </>
              }
            >
              {['syncing', 'streaming', 'stalled'].includes(r.state) ? (
                <Button variant="danger-quiet" disabled={!canMutate} onClick={() => setAsk({ kind: 'abort', db: r.db, from: r.from, to: r.to })}>
                  Stop the move…
                </Button>
              ) : null}
              {r.state === 'streaming' ? (
                <Button variant="accent" disabled={!canMutate} onClick={() => setAsk({ kind: 'switch', db: r.db, from: r.from, to: r.to })}>
                  Switch now…
                </Button>
              ) : null}
            </SettingRow>
          )
        })}
        {live.notLogical.map((m) => (
          <SettingRow
            key={m}
            testId={`data-live-prepare-${m}`}
            label={`Postgres ${m} isn’t ready for moves yet`}
            hint={
              live.helper
                ? `Getting it ready restarts Postgres ${m} once: its databases pause for about 5 to 15 seconds, after a minute’s notice.`
                : 'This server’s setup is too old to move databases while they work. Update the server, then come back.'
            }
          >
            <Button disabled={!canMutate || !live.helper} onClick={() => setAsk({ kind: 'prepare', major: m })}>
              Get it ready…
            </Button>
          </SettingRow>
        ))}
      </SettingsGroup>

      {ask?.kind === 'switch' ? (
        <ActionSheet
          testId="data-live-switch-confirm"
          title={`Switch ${ask.db} to Postgres ${ask.to}?`}
          message={`Changes to ${ask.db} pause for a few seconds while ${PRODUCT_NAME} switches over; reading keeps working. After that, ${ask.db} runs on Postgres ${ask.to}.`}
          confirmLabel="Switch now"
          busyLabel="Switching…"
          failWhat={`Couldn’t switch ${ask.db}.`}
          failNext={`It’s still on Postgres ${ask.from} and working. Try again in a moment.`}
          refusedWhat={`Only the server’s owner or an admin can switch ${ask.db}.`}
          run={() => switchLiveMove(ask.db)}
          onDone={() => {
            setAsk(null)
            setNotice({ tone: 'ok', text: `${ask.db} now runs on Postgres ${ask.to}.` })
            void load()
          }}
          onClose={() => setAsk(null)}
        />
      ) : null}
      {ask?.kind === 'abort' ? (
        <ActionSheet
          testId="data-live-abort-confirm"
          destructive
          title={`Stop moving ${ask.db}?`}
          message={`${ask.db} stays on Postgres ${ask.from} and keeps working. You can start the move again later.`}
          confirmLabel="Stop the move"
          busyLabel="Stopping…"
          failWhat="Couldn’t stop the move."
          failNext="The move is still running. Try again in a moment."
          refusedWhat={`Only the server’s owner or an admin can stop moving ${ask.db}.`}
          run={() => abortLiveMove(ask.db)}
          onDone={() => {
            setAsk(null)
            setNotice({ tone: 'ok', text: `Stopped moving ${ask.db}. It stays on Postgres ${ask.from}.` })
            void load()
          }}
          onClose={() => setAsk(null)}
        />
      ) : null}
      {ask?.kind === 'prepare' ? (
        <ActionSheet
          testId="data-live-prepare-confirm"
          title={`Get Postgres ${ask.major} ready for moves?`}
          message={`Postgres ${ask.major} restarts once. Every database on it pauses for about 5 to 15 seconds; agents and apps get a minute’s notice first.`}
          confirmLabel={`Restart Postgres ${ask.major}`}
          busyLabel="Restarting…"
          failWhat={`Couldn’t restart Postgres ${ask.major}.`}
          failNext="Nothing changed. Try again in a moment."
          refusedWhat={`Only the server’s owner or an admin can restart Postgres ${ask.major}.`}
          run={() => prepareLiveMoves(ask.major)}
          onDone={() => {
            setAsk(null)
            setNotice({ tone: 'ok', text: `Postgres ${ask.major} is ready: its databases can move to a newer version while they work.` })
            void load()
          }}
          onClose={() => setAsk(null)}
        />
      ) : null}
    </>
  )
}

function ServerTab({
  status,
  canMutate,
  sample,
  onChanged,
  setNotice,
}: {
  status: SqlStatus
  canMutate: boolean
  sample: boolean
  onChanged: () => void
  setNotice: (n: Notice | null) => void
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [askOff, setAskOff] = useState(false)
  const notInstalled = status.state === 'not-installed'
  const off = status.state === 'disabled' || status.state === 'stopped'
  const listen = formatSqlListen(status.listen, status.port)
  const { word, tone } = sqlStateWord(status.state)
  const clusters = status.clusters ?? []

  const turnOn = useCallback(async (): Promise<void> => {
    if (sample) return
    setBusy(true)
    setNotice(null)
    const what = notInstalled ? 'set up' : 'turn on'
    try {
      await enableSqlServer()
      setNotice({
        tone: 'ok',
        text: notInstalled
          ? 'The database server is set up and running. Each workspace can now have its own database.'
          : 'The database server is on. Agents and apps can use their databases again.',
      })
      onChanged()
    } catch (e) {
      if (sqlNotBaked(e)) {
        // The daemon's hint is a setup command; say it in words instead.
        setNotice({ tone: 'error', text: 'Postgres isn’t installed on this server, so there’s nothing to start yet. Install it with the server’s setup (the database option), then come back and choose Set up.' })
      } else if (sqlRefused(e)) {
        setNotice({ tone: 'error', text: `Only the server’s owner or an admin can ${what} the database server.` })
      } else {
        setNotice({ tone: 'error', text: `Couldn’t ${what} the database server. Nothing changed; try again in a moment.`, detail: sqlErrorMessage(e) })
      }
    } finally {
      setBusy(false)
    }
  }, [notInstalled, onChanged, sample, setNotice])

  if (notInstalled || off) {
    return (
      <SettingsGroup
        card
        title="Server"
        settingId="data.server"
        state={
          <SettingsLoadState
            kind="empty"
            testId={notInstalled ? 'db-not-installed' : 'db-off'}
            title={notInstalled ? 'The database server isn’t set up' : 'The database server is off'}
            action={
              <Button variant="accent" disabled={!canMutate || busy} aria-busy={busy || undefined} onClick={() => void turnOn()} data-testid="db-turn-on">
                {notInstalled ? (busy ? 'Setting up…' : 'Set up') : busy ? 'Turning on…' : 'Turn on'}
              </Button>
            }
          >
            {notInstalled
              ? `Setting it up starts the Postgres that came with this server, so each workspace can have its own database. If the server was set up without Postgres, ${PRODUCT_NAME} says so and nothing changes.`
              : 'Your databases and their data are kept. Turn it on to use them again.'}
          </SettingsLoadState>
        }
      />
    )
  }

  return (
    <div className="space-y-6">
      <SettingsGroup card title="Server" settingId="data.server" testId="db-server">
        <SettingRow
          label="Status"
          hint={statusSentence(status)}
          below={
            status.lastError ? (
              <p className={SETTING_SERVER_REASON} data-testid="db-last-error">
                The server said: {status.lastError.replace(/[.\s]+$/, '')}.
              </p>
            ) : undefined
          }
        >
          <span data-testid="db-state">
            <StatusWord tone={tone}>{word}</StatusWord>
          </span>
        </SettingRow>
        {clusters.length > 1 ? (
          <SettingRow label="Versions" hint="Each database stays on its own version until you move it.">
            <span className="type-row text-[var(--color-text-secondary)]">
              Postgres {clusters.map((c) => c.major).join(' and ')}
            </span>
          </SettingRow>
        ) : null}
        {listen ? (
          <SettingRow
            label="Address"
            hint="Only agents and apps on this server can connect. To reach a database from another computer, ask an agent to publish it on a web address."
          >
            <CopyValue value={listen} label="database address" />
          </SettingRow>
        ) : null}
      </SettingsGroup>

      {status.state === 'running' && !sample ? <LiveMovesGroup canMutate={canMutate} setNotice={setNotice} /> : null}

      <SettingsGroup card title="Turn off">
        <SettingRow
          label="Turn off the database server"
          hint="Stops Postgres. Agents and apps can’t use their databases until you turn it back on. Every database and its data is kept."
        >
          <Button variant="danger-quiet" disabled={!canMutate || busy} onClick={() => setAskOff(true)} data-testid="db-turn-off">
            Turn off…
          </Button>
        </SettingRow>
      </SettingsGroup>

      {askOff ? (
        <ActionSheet
          testId="db-turn-off-confirm"
          destructive
          title="Turn off the database server?"
          message="Agents and apps can’t use their databases until you turn it back on. Every database and its data is kept."
          confirmLabel="Turn off"
          busyLabel="Turning off…"
          failWhat="Couldn’t turn off the database server."
          failNext="It’s still running. Try again in a moment."
          refusedWhat="Only the server’s owner or an admin can turn the database server off."
          run={() => disableSqlServer()}
          onDone={() => {
            setAskOff(false)
            setNotice({ tone: 'ok', text: 'The database server is off. Your databases and their data are kept; turn it on again here.' })
            onChanged()
          }}
          onClose={() => setAskOff(false)}
        />
      ) : null}
    </div>
  )
}

// ── Databases tab ─────────────────────────────────────────────────────────

function CreateDatabaseDialog({
  databases,
  onClose,
  onCreated,
}: {
  databases: SqlDatabase[]
  onClose: () => void
  onCreated: (workspaceId: string, name: string | null) => void
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const [ws, setWs] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const formId = useId()
  const [refused, setRefused] = useState(false)
  const owners = useMemo(() => new Set(databases.map((d) => d.ownerProjectId)), [databases])
  const options = useMemo(
    () => projects.map((p) => ({ value: p.id, label: p.name || p.path, badge: owners.has(p.id) ? 'Has one' : undefined })),
    [projects, owners],
  )

  const submit = async (): Promise<void> => {
    if (!ws || busy) return
    setBusy(true)
    setError(null)
    try {
      const res = await createSqlDatabase(ws)
      onCreated(ws, res?.name ?? null)
    } catch (e) {
      setRefused(sqlRefused(e))
      setError(sqlErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <AccessSheet
      title="Create a database"
      intro="A workspace’s agents and apps keep their data in its database. Only that workspace can use it until you give others access."
      onClose={onClose}
      busy={busy}
      testId="db-create-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            {refused ? 'Close' : 'Cancel'}
          </Button>
          {refused ? null : (
            <Button type="submit" form={formId} variant="accent" size="md" disabled={!ws || busy} aria-busy={busy || undefined} data-testid="db-create-submit">
              {busy ? 'Creating…' : 'Create database'}
            </Button>
          )}
        </>
      }
    >
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <div>
          <p className={DIALOG_LABEL}>Workspace</p>
          <SettingDropdown
            ariaLabel="Workspace"
            value={ws}
            placeholder="Choose a workspace"
            options={[{ value: '', label: 'Choose a workspace' }, ...options]}
            onChange={(v) => {
              setWs(v)
              setError(null)
              setRefused(false)
            }}
            menuAlign="left"
            fullWidth
          />
          <p className={`${DIALOG_MUTED} mt-1`}>The database belongs to this workspace. Its agents can read and change everything in it.</p>
        </div>
        {error ? (
          refused ? (
            <FailedNote what="Only the server’s owner or an admin can create databases." detail={error} next="Nothing was created." />
          ) : (
            <FailedNote what="Couldn’t create the database." detail={error} next="Nothing was created. Try again, or choose another workspace." />
          )
        ) : null}
      </form>
    </AccessSheet>
  )
}

function GiveAccessDialog({
  db,
  onClose,
  onGiven,
}: {
  db: SqlDatabase
  onClose: () => void
  onGiven: (g: SqlParticipant) => void
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const [ws, setWs] = useState('')
  const [level, setLevel] = useState<SqlLevel>('read')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [refused, setRefused] = useState(false)
  const formId = useId()
  const taken = useMemo(() => new Set([db.ownerProjectId, ...db.grants.map((g) => g.projectId)]), [db])
  const options = useMemo(() => projects.filter((p) => !taken.has(p.id)).map((p) => ({ value: p.id, label: p.name || p.path })), [projects, taken])

  const submit = async (): Promise<void> => {
    if (!ws || busy) return
    setBusy(true)
    setError(null)
    try {
      await grantSqlAccess({ project: ws, db: db.id, level })
      onGiven({ projectId: ws, workspace: projects.find((p) => p.id === ws)?.name ?? null, level, canManage: false })
    } catch (e) {
      setRefused(sqlRefused(e))
      setError(sqlErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <AccessSheet
      title={`Give access to ${db.name}`}
      intro="Agents and apps in the workspace you choose can use this database. You can change or remove this later."
      onClose={onClose}
      busy={busy}
      testId="db-grant-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            {refused ? 'Close' : 'Cancel'}
          </Button>
          {refused ? null : (
            <Button type="submit" form={formId} variant="accent" size="md" disabled={!ws || busy} aria-busy={busy || undefined} data-testid="db-grant-submit">
              {busy ? 'Giving access…' : 'Give access'}
            </Button>
          )}
        </>
      }
    >
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <div>
          <p className={DIALOG_LABEL}>Workspace</p>
          {options.length === 0 ? (
            <p className={DIALOG_MUTED}>Every workspace already has access.</p>
          ) : (
            <SettingDropdown
              ariaLabel="Workspace"
              value={ws}
              placeholder="Choose a workspace"
              options={[{ value: '', label: 'Choose a workspace' }, ...options]}
              onChange={setWs}
              menuAlign="left"
              fullWidth
            />
          )}
        </div>
        <div>
          <p className={DIALOG_LABEL}>What it can do</p>
          <SettingDropdown ariaLabel="What it can do" value={level} options={LEVEL_OPTIONS} onChange={(v) => setLevel(v as SqlLevel)} menuAlign="left" fullWidth />
        </div>
        {error ? (
          refused ? (
            <FailedNote what={`Only the workspace’s owner can change access to ${db.name}.`} detail={error} next="Nothing changed." />
          ) : (
            <FailedNote what="Couldn’t give access." detail={error} next="Nothing changed. Try again in a moment." />
          )
        ) : null}
      </form>
    </AccessSheet>
  )
}

function GrantRow({
  db,
  grant,
  name,
  canMutate,
  patchDb,
  onRemove,
}: {
  db: SqlDatabase
  grant: SqlParticipant
  name: string
  canMutate: boolean
  patchDb: (id: string, patch: (row: SqlDatabase) => SqlDatabase) => void
  onRemove: () => void
}): React.JSX.Element {
  const save = useSaveStatus()
  const [busy, setBusy] = useState(false)
  const [refused, setRefused] = useState(false)

  const change = async (level: SqlLevel, canManage: boolean): Promise<void> => {
    if (busy) return
    setBusy(true)
    setRefused(false)
    const before = db.grants
    patchDb(db.id, (row) => ({ ...row, grants: row.grants.map((g) => (g.projectId === grant.projectId ? { ...g, level, canManage } : g)) }))
    try {
      await grantSqlAccess({ project: grant.projectId, db: db.id, level, manage: canManage })
      save.saved()
    } catch (e) {
      patchDb(db.id, (row) => ({ ...row, grants: before }))
      if (sqlRefused(e)) {
        save.reset()
        setRefused(true)
      } else {
        save.failed(new Error(sqlErrorMessage(e)))
      }
    } finally {
      setBusy(false)
    }
  }

  const can = grant.level === 'write' ? 'Can read and change it' : 'Can read it'
  return (
    <SettingRow
      testId="db-grant-row"
      label={name}
      hint={
        <>
          {can}
          {grant.canManage ? ', and give other workspaces access.' : '.'}
          <SaveError state={save.state} />
          {refused ? (
            <p role="alert" data-testid="db-grant-refused" className="type-hint text-[var(--color-status-error-soft)] mt-1 leading-relaxed max-w-[75ch]">
              Only the workspace’s owner can change access to {db.name}. Nothing changed.
            </p>
          ) : null}
        </>
      }
    >
      <SaveNote state={save.state} />
      <SettingDropdown
        ariaLabel={`What ${name} can do`}
        value={grant.level}
        options={LEVEL_OPTIONS}
        disabled={!canMutate || busy}
        onChange={(next) => void change(next as SqlLevel, grant.canManage)}
      />
      <span className="flex items-center gap-2">
        <Toggle
          size="sm"
          checked={grant.canManage}
          disabled={!canMutate || busy}
          onChange={(next) => void change(grant.level, next)}
          aria-label={`${name} can manage access`}
        />
        <span className="type-hint text-[var(--color-text-secondary)]" aria-hidden="true">
          Manage
        </span>
      </span>
      <Button variant="danger-quiet" disabled={!canMutate || busy} onClick={onRemove} aria-label={`Remove ${name}’s access`}>
        Remove…
      </Button>
    </SettingRow>
  )
}

function RoleGroup({
  db,
  canMutate,
  patchDb,
}: {
  db: SqlDatabase
  canMutate: boolean
  patchDb: (id: string, patch: (row: SqlDatabase) => SqlDatabase) => void
}): React.JSX.Element {
  const [draft, setDraft] = useState(db.bindRole ?? '')
  const [busy, setBusy] = useState(false)
  const [refused, setRefused] = useState(false)
  const save = useSaveStatus()
  const inputId = useId()

  useEffect(() => {
    setDraft(db.bindRole ?? '')
  }, [db.bindRole, db.id])

  const changed = draft.trim() !== '' && draft.trim() !== (db.bindRole ?? '')
  const submit = async (): Promise<void> => {
    const role = draft.trim()
    if (!role || busy) return
    setBusy(true)
    setRefused(false)
    const before = db.bindRole
    patchDb(db.id, (row) => ({ ...row, bindRole: role }))
    try {
      const res = await bindSqlRole({ project: db.ownerProjectId, db: db.id, role })
      if (res?.bindRole) patchDb(db.id, (row) => ({ ...row, bindRole: res.bindRole ?? role }))
      save.saved()
    } catch (e) {
      patchDb(db.id, (row) => ({ ...row, bindRole: before }))
      setDraft(before ?? '')
      if (sqlRefused(e)) {
        save.reset()
        setRefused(true)
      } else {
        save.failed(new Error(sqlErrorMessage(e)))
      }
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingsGroup card title="Agent sign-in" settingId="data.bind">
      <SettingRow
        label={<label htmlFor={inputId}>Sign in as</label>}
        hint={
          <>
            The Postgres role the owner workspace’s agents use. Leave it as it is unless an app needs its own role.
            <SaveError state={save.state} />
            {refused ? (
              <p role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-1 leading-relaxed max-w-[75ch]">
                Only the server’s owner or an admin can change the sign-in role. Nothing changed.
              </p>
            ) : null}
          </>
        }
      >
        <SaveNote state={save.state} />
        <Input
          id={inputId}
          type="text"
          value={draft}
          disabled={!canMutate || busy}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void submit()
          }}
          placeholder="Role name"
          className="w-48 type-control font-mono"
        />
        <Button disabled={!canMutate || busy || !changed} onClick={() => void submit()}>
          {busy ? 'Saving…' : 'Save'}
        </Button>
      </SettingRow>
    </SettingsGroup>
  )
}

function DatabaseDetail({
  db,
  status,
  canMutate,
  patchDb,
  onBack,
  setNotice,
}: {
  db: SqlDatabase
  status: SqlStatus
  canMutate: boolean
  patchDb: (id: string, patch: (row: SqlDatabase) => SqlDatabase) => void
  onBack: () => void
  setNotice: (n: Notice | null) => void
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const names = useMemo(() => new Map(projects.map((p) => [p.id, p.name])), [projects])
  const [dialog, setDialog] = useState<{ kind: 'grant' } | { kind: 'remove'; grant: SqlParticipant } | null>(null)
  const owner = participantName(db.owner, names)
  const facts = [`Owned by ${owner}`, dbVersionWord(status, db.id), fmtDay(db.createdAt) ? `created ${fmtDay(db.createdAt)}` : null].filter(Boolean).join(' · ')

  return (
    <div className="space-y-5" data-testid="db-detail">
      <div className="space-y-2">
        <button
          type="button"
          data-k2-focus=""
          onClick={onBack}
          className="relative inline-flex items-center gap-1 type-hint text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] cursor-pointer outline-none no-drag"
          data-testid="db-back"
        >
          <svg aria-hidden="true" viewBox="0 0 16 16" className="w-3 h-3" fill="none" stroke="currentColor" strokeWidth={1.75}>
            <path d="M10 3.5 5.5 8 10 12.5" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
          All databases
        </button>
        <div className="flex items-center gap-3 min-w-0">
          <h3 className="type-heading text-[var(--color-text-primary)] truncate">{db.name}</h3>
          <StatusWord tone={dbStatusWord(db.status).tone}>{dbStatusWord(db.status).word}</StatusWord>
        </div>
        <p className={SETTING_ROW_HINT}>{facts}.</p>
      </div>

      <SettingsGroup
        card
        title="Who can use it"
        hint="The owner workspace’s agents can always read and change it. Manage lets another workspace give access too."
        settingId="data.grants"
        action={
          <Button disabled={!canMutate} onClick={() => setDialog({ kind: 'grant' })} data-testid="db-grant-open">
            Give access…
          </Button>
        }
      >
        <SettingRow label={owner} hint="Owner. Can read and change it, and give other workspaces access." testId="db-owner-row">
          <span className="type-hint text-[var(--color-text-secondary)]">Owner</span>
        </SettingRow>
        {db.grants.map((g) => (
          <GrantRow
            key={g.projectId}
            db={db}
            grant={g}
            name={participantName(g, names)}
            canMutate={canMutate}
            patchDb={patchDb}
            onRemove={() => setDialog({ kind: 'remove', grant: g })}
          />
        ))}
        {db.grants.length === 0 ? (
          <div className="px-4 py-3">
            <p className={SETTING_ROW_HINT}>No other workspace can use it. Give access to share it.</p>
          </div>
        ) : null}
      </SettingsGroup>

      <RoleGroup db={db} canMutate={canMutate} patchDb={patchDb} />

      {dialog?.kind === 'grant' ? (
        <GiveAccessDialog
          db={db}
          onClose={() => setDialog(null)}
          onGiven={(g) => {
            setDialog(null)
            patchDb(db.id, (row) => ({ ...row, grants: [...row.grants, g] }))
            setNotice({ tone: 'ok', text: `${participantName(g, names)} can now ${g.level === 'write' ? 'read and change' : 'read'} ${db.name}.` })
          }}
        />
      ) : null}
      {dialog?.kind === 'remove' ? (
        <ActionSheet
          testId="db-remove-confirm"
          destructive
          title={`Remove ${participantName(dialog.grant, names)}’s access?`}
          message={`Agents and apps in ${participantName(dialog.grant, names)} can no longer use ${db.name}. Nothing in the database is deleted, and you can give access again later.`}
          confirmLabel="Remove access"
          busyLabel="Removing…"
          failWhat="Couldn’t remove access."
          failNext="They still have access. Try again in a moment."
          refusedWhat={`Only the workspace’s owner can change access to ${db.name}.`}
          run={() => revokeSqlAccess({ project: dialog.grant.projectId, db: db.id })}
          onDone={() => {
            const gone = dialog.grant
            setDialog(null)
            patchDb(db.id, (row) => ({ ...row, grants: row.grants.filter((x) => x.projectId !== gone.projectId) }))
            setNotice({ tone: 'ok', text: `Removed ${participantName(gone, names)}’s access to ${db.name}.` })
          }}
          onClose={() => setDialog(null)}
        />
      ) : null}
    </div>
  )
}

function DatabaseList({
  databases,
  listError,
  status,
  running,
  canMutate,
  onOpen,
  onCreate,
  onRetry,
  onServerTab,
}: {
  databases: SqlDatabase[] | null
  listError: string | null
  status: SqlStatus
  running: boolean
  canMutate: boolean
  onOpen: (id: string) => void
  onCreate: () => void
  onRetry: () => void
  onServerTab: () => void
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const names = useMemo(() => new Map(projects.map((p) => [p.id, p.name])), [projects])
  const state = !running ? (
    <SettingsLoadState kind="empty" title="The database server isn’t running" testId="db-server-off" action={<Button onClick={onServerTab}>Open Server</Button>}>
      Databases are kept, but can’t be used or created until the server runs. Turn it on from Server.
    </SettingsLoadState>
  ) : databases === null ? (
    <SettingsLoadState kind="loading" title="Loading databases…" testId="db-list-loading">
      Reading the databases on this server.
    </SettingsLoadState>
  ) : listError ? (
    <SettingsLoadState kind="error" title="Couldn’t load the databases" detail={listError} action={<Button onClick={onRetry}>Try again</Button>} testId="db-list-error">
      Your databases are safe; this page just couldn’t read the list. Try again in a moment.
    </SettingsLoadState>
  ) : databases.length === 0 ? (
    <SettingsLoadState kind="empty" title="No databases yet" testId="db-list-empty">
      Create one for a workspace, and its agents and apps can keep their data in it.
    </SettingsLoadState>
  ) : undefined

  return (
    <SettingsGroup
      card
      title="Databases"
      hint="Each workspace can have its own database. Open one to choose who else can use it."
      settingId="data.databases"
      action={
        <Button variant="accent" disabled={!canMutate || !running} onClick={onCreate} data-testid="db-create-open">
          Create database…
        </Button>
      }
      state={state}
    >
      {(databases ?? []).map((d) => {
        const owner = participantName(d.owner, names)
        const shared = d.grants.length === 0 ? 'only its owner uses it' : d.grants.length === 1 ? 'shared with 1 workspace' : `shared with ${d.grants.length} workspaces`
        const facts = [`Owned by ${owner}`, dbVersionWord(status, d.id), shared].filter(Boolean).join(' · ')
        return (
          <button
            key={d.id}
            type="button"
            data-testid="db-row"
            data-db={d.name}
            data-k2-focus=""
            onClick={() => onOpen(d.id)}
            aria-label={`Open ${d.name}`}
            className={`relative w-full text-left px-4 py-3 flex items-center justify-between gap-6 cursor-pointer outline-none no-drag ${SETTING_LIST_ROW_HOVER}`}
          >
            <span className="min-w-0">
              <span className="block type-name text-[var(--color-text-primary)] truncate">{d.name}</span>
              <span className="block type-hint text-[var(--color-text-muted)] mt-0.5 truncate">{facts}</span>
            </span>
            <span className="flex items-center gap-3 flex-shrink-0">
              <StatusWord tone={dbStatusWord(d.status).tone}>{dbStatusWord(d.status).word}</StatusWord>
              <svg aria-hidden="true" viewBox="0 0 16 16" className="w-3.5 h-3.5 text-[var(--color-text-secondary)]" fill="none" stroke="currentColor" strokeWidth={1.75}>
                <path d="M6 3.5 10.5 8 6 12.5" strokeLinecap="round" strokeLinejoin="round" />
              </svg>
            </span>
          </button>
        )
      })}
    </SettingsGroup>
  )
}

// ── The page ──────────────────────────────────────────────────────────────

export function DataSection(): React.JSX.Element {
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')

  const [status, setStatus] = useState<SqlStatus | null>(null)
  const [statusError, setStatusError] = useState<string | null>(null)
  const [databases, setDatabases] = useState<SqlDatabase[] | null>(null)
  const [listError, setListError] = useState<string | null>(null)
  const [tab, setTab] = useState<Tab>('databases')
  const [openId, setOpenId] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const [notice, setNotice] = useState<Notice | null>(null)
  const [revision, setRevision] = useState(0)
  const bump = useCallback(() => setRevision((r) => r + 1), [])

  const sample = status !== null && !status.supported
  const canMutate = status !== null && status.supported && !viewerReadOnly
  const supported: boolean | null = status === null ? null : status.supported

  useEffect(() => {
    let cancelled = false
    fetchSqlStatus()
      .then((s) => {
        if (cancelled) return
        setStatus(s)
        setStatusError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setStatusError(sqlErrorMessage(e))
      })
    return () => {
      cancelled = true
    }
  }, [revision])

  useEffect(() => {
    if (supported === null) return
    if (!supported) {
      setDatabases(SAMPLE_DATABASES)
      setListError(null)
      return
    }
    let cancelled = false
    fetchSqlDatabases()
      .then((rows) => {
        if (cancelled) return
        setDatabases(rows)
        setListError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setDatabases([])
        setListError(sqlErrorMessage(e))
      })
    return () => {
      cancelled = true
    }
  }, [supported, revision])

  // A database that went away (dropped elsewhere) closes back to the list.
  useEffect(() => {
    if (openId && databases && !databases.some((d) => d.id === openId)) setOpenId(null)
  }, [databases, openId])

  const patchDb = useCallback((id: string, patch: (row: SqlDatabase) => SqlDatabase): void => {
    setDatabases((prev) => prev?.map((row) => (row.id === id ? patch(row) : row)) ?? null)
  }, [])

  const retryStatus = useCallback(() => {
    setStatusError(null)
    setStatus(null)
    bump()
  }, [bump])

  const header = (
    <header className="min-w-0">
      <h2 className="type-title text-[var(--color-text-primary)]">Database</h2>
      <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed max-w-[75ch]">
        Postgres databases for your workspaces. Each workspace can have its own, for its agents and apps, and you choose which other
        workspaces can use it.
      </p>
      {viewerReadOnly ? (
        <p className={SETTING_ROW_HINT} data-testid="db-viewer-note">
          You’re viewing this server. Only its owner can change these settings.
        </p>
      ) : null}
    </header>
  )

  const frame = (body: React.ReactNode): React.JSX.Element => (
    <div className="h-full min-h-0 overflow-y-auto p-6">
      <div className="w-full max-w-[52rem] space-y-5 pb-6">{body}</div>
    </div>
  )

  if (statusError) {
    return frame(
      <>
        {header}
        <SettingsLoadState kind="error" title="Couldn’t read the database server’s status" detail={statusError} action={<Button onClick={retryStatus}>Try again</Button>} testId="db-status-error">
          Your databases are safe; this page just couldn’t reach the server. Try again in a moment.
        </SettingsLoadState>
      </>,
    )
  }
  if (status === null) {
    return frame(
      <>
        {header}
        <SettingsLoadState kind="loading" title="Checking the database server…" testId="db-status-loading">
          Reading its status and your databases.
        </SettingsLoadState>
      </>,
    )
  }

  // On a server that can't run it, the whole page is the sample.
  const view: SqlStatus = sample ? SAMPLE_STATUS : status
  const running = view.state === 'running' || view.state === 'degraded'
  const openDb = openId ? (databases ?? []).find((d) => d.id === openId) : undefined
  const tabs: Tab[] = ['databases', 'server']

  return frame(
    <>
      {header}
      {sample ? <LinuxOnlyNote what="Database hosting" /> : null}
      {notice ? (
        <Callout
          tone={notice.tone}
          role={notice.tone === 'error' ? 'alert' : 'status'}
          className="type-hint flex items-start justify-between gap-4"
          data-testid="db-notice"
        >
          <div className="min-w-0">
            <p>{notice.text}</p>
            {notice.detail ? <p className="mt-0.5">The server said: {notice.detail.trim().replace(/[.\s]+$/, '')}.</p> : null}
          </div>
          <Button variant="quiet" className="flex-shrink-0" onClick={() => setNotice(null)}>
            Dismiss
          </Button>
        </Callout>
      ) : null}

      <div role="tablist" aria-label="Database" className="flex flex-wrap gap-0.5 border-b border-[var(--color-border)] w-full">
        {tabs.map((t) => {
          const active = tab === t
          return (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={active}
              onClick={() => {
                setTab(t)
                setOpenId(null)
              }}
              data-settings-id={TAB_SETTINGS_ID[t]}
              data-k2-focus=""
              className={`relative px-3 py-2 type-control transition-colors no-drag cursor-pointer border-b-2 -mb-px outline-none ${
                active
                  ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                  : 'border-transparent text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
              }`}
            >
              {TAB_LABEL[t]}
            </button>
          )
        })}
      </div>

      <div className="pt-1">
        {tab === 'server' ? (
          <ServerTab status={view} canMutate={canMutate} sample={sample} onChanged={bump} setNotice={setNotice} />
        ) : openDb ? (
          <DatabaseDetail db={openDb} status={view} canMutate={canMutate} patchDb={patchDb} onBack={() => setOpenId(null)} setNotice={setNotice} />
        ) : (
          <DatabaseList
            databases={databases}
            listError={listError}
            status={view}
            running={running}
            canMutate={canMutate}
            onOpen={(id) => {
              setNotice(null)
              setOpenId(id)
            }}
            onCreate={() => setCreating(true)}
            onRetry={() => {
              setDatabases(null)
              bump()
            }}
            onServerTab={() => setTab('server')}
          />
        )}
      </div>

      {creating ? (
        <CreateDatabaseDialog
          databases={databases ?? []}
          onClose={() => setCreating(false)}
          onCreated={(wsId, name) => {
            setCreating(false)
            const ws = useProjectsStore.getState().projects.find((p) => p.id === wsId)?.name ?? 'the workspace'
            setNotice({ tone: 'ok', text: `Created ${name ?? 'a database'} for ${ws}. Its agents and apps can use it now.` })
            bump()
          }}
        />
      ) : null}
    </>,
  )
}
