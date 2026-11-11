// Settings → API Keys dialogs: Create API key (then the key, once, with
// Copy), and a hire key's App access. Same frame, fields and failure note as
// Admin Access (AccessDialogs.tsx). A new key's secret lives only in the open
// dialog: closing drops it.

import React, { useEffect, useId, useMemo, useRef, useState } from 'react'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import { currentShortcutOs } from '@/lib/shortcut-label'
import type { DesktopOs } from '@/lib/desktop-chrome'
import { Button, Callout, Checkbox, Input, Radio, Spinner, Surface } from '@/components/ui'
import { SETTING_ROW_LABEL, SettingDropdown } from '../controls/SettingControls'
import { AccessSheet, CopyLine, DIALOG_FIELD, DIALOG_LABEL, DIALOG_MUTED, Field } from './AccessDialogs'
import {
  ceilingFormFromLimits,
  ceilingFromForm,
  ceilingSummary,
  createApiKey,
  EMPTY_CEILING_FORM,
  finishCeilingConfirm,
  finishHireKeyConfirm,
  HIRE_FORM_DEFAULTS,
  hireLimitsFromForm,
  KEY_DOORS,
  keyName,
  NO_APP_ACCESS,
  saveHireCeiling,
  SELF_ROOM,
  startHireKeyConfirm,
  type ApiKeyRow,
  type CeilingForm,
  type CeilingOptions,
  type CeilingRoom,
  type HireForm,
  type KeyDoor,
} from './api-keys-api'

/** The hire-key confirm, in the create form. Touch ID (`local-auth`) exists
 *  only for the owner on a Mac; Linux and Windows confirm with a password
 *  (the Connect login password or the vault password, `vault_routes.rs`
 *  `methods`). */
export function hireConfirmCopy(os: DesktopOs = currentShortcutOs()): string {
  return os === 'mac'
    ? 'Creating it asks you to confirm it’s you, with Touch ID or a password.'
    : 'Creating it asks for your password to confirm it’s you.'
}

/** A failure inside a dialog: what failed, what to do now, and the server's
 *  reason quietly when there is one. */
type Problem = { what: string; next: string; detail: string | null }

function ProblemNote({ problem }: { problem: Problem }): React.JSX.Element {
  // At the foot of a long form, just above the buttons: scrolled into view
  // (the dialog's title and buttons stay put), so the failure is seen.
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    ref.current?.scrollIntoView?.({ block: 'nearest' })
  }, [problem])
  return (
    <div ref={ref}>
      <Callout tone="error" role="alert" className="type-hint space-y-0.5" data-testid="dialog-error">
        <p className="font-medium">{problem.what}</p>
        <p>{problem.next}</p>
        {problem.detail ? <p>The server said: {problem.detail.trim().replace(/[.\s]+$/, '')}.</p> : null}
      </Callout>
    </div>
  )
}

/** How many rows a pick list shows before "Show all": the list grows with
 *  the dialog's own scroll, never a scroll box of its own. */
const LIST_CAP = 5

/** The rows to show: the first `LIST_CAP`, plus any ticked one further
 *  down (a choice is never hidden), or every row once opened. */
function useShowAll<T>(items: T[], ticked: (item: T) => boolean): { shown: T[]; hidden: number; all: boolean; setAll: (v: boolean) => void } {
  const [all, setAll] = useState(false)
  if (all || items.length <= LIST_CAP + 1) return { shown: items, hidden: 0, all, setAll }
  const shown = items.filter((item, i) => i < LIST_CAP || ticked(item))
  return { shown, hidden: items.length - shown.length, all, setAll }
}

/** The last row of a capped pick list: Show all N, or Show fewer. */
function ShowAllRow({ list, total, noun }: { list: { hidden: number; all: boolean; setAll: (v: boolean) => void }; total: number; noun: string }): React.JSX.Element | null {
  if (!list.all && list.hidden === 0) return null
  return (
    <button
      type="button"
      data-k2-focus=""
      aria-expanded={list.all}
      className="relative block w-full text-left px-3 py-2 type-row text-[var(--color-accent-text)] hover:bg-[var(--color-wash-1)] no-drag cursor-pointer"
      onClick={() => list.setAll(!list.all)}
    >
      {list.all ? 'Show fewer' : `Show all ${total} ${noun}`}
    </button>
  )
}

/** The first problem marked in a form, scrolled into view. */
function scrollToFirstProblem(form: HTMLFormElement | null): void {
  form?.querySelector('[data-field-problem]')?.scrollIntoView?.({ block: 'nearest' })
}

/** Thrown when this computer has no way to confirm: no Touch ID and no
 *  password method. Said in the page's words, with no server text. */
class NoConfirmMethod extends Error {}

/**
 * The owner confirm (Touch ID / login password, or the vault password) for
 * a hire-key mint or an App access raise. `begin(then)` runs `then(token)`
 * right away on a Mac, or after the password step. `stage` says which part
 * a failure came from, so the dialog can say "couldn't confirm" or
 * "couldn't create".
 */
function useOwnerConfirm(finish: (method: string, password?: string) => Promise<{ token: string }>) {
  const [step, setStep] = useState<'idle' | 'mac' | 'password'>('idle')
  const [pwMethod, setPwMethod] = useState('vault-password')
  const [password, setPassword] = useState('')
  const stage = useRef<'confirm' | 'write'>('write')
  const pending = useRef<((token: string) => Promise<void>) | null>(null)

  const begin = async (then: (token: string) => Promise<void>): Promise<void> => {
    stage.current = 'confirm'
    try {
      const { methods } = await startHireKeyConfirm()
      if (methods.includes('local-auth')) {
        setStep('mac')
        const { token } = await finish('local-auth')
        setStep('idle')
        stage.current = 'write'
        await then(token)
        return
      }
      const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
      if (pw) {
        pending.current = then
        setPwMethod(pw)
        setStep('password')
        return
      }
    } catch (e) {
      setStep('idle')
      throw e
    }
    throw new NoConfirmMethod('no Touch ID or password to confirm with')
  }

  const submitPassword = async (): Promise<void> => {
    stage.current = 'confirm'
    const pw = password
    setPassword('')
    const { token } = await finish(pwMethod, pw)
    setStep('idle')
    stage.current = 'write'
    const then = pending.current
    pending.current = null
    if (then) await then(token)
  }

  return { step, pwMethod, password, setPassword, stage, begin, submitPassword }
}

type OwnerConfirm = ReturnType<typeof useOwnerConfirm>

/** The confirm failure in plain words, or null when it wasn't the confirm. */
function confirmProblem(confirm: OwnerConfirm, e: unknown): Problem | null {
  if (e instanceof NoConfirmMethod) {
    return {
      what: 'K2 can’t confirm it’s you on this computer yet.',
      next: 'There’s no Touch ID or password to confirm with. Set a vault password from a terminal outside K2 (k2 vault confirm-password set), then try again.',
      detail: null,
    }
  }
  if (confirm.stage.current !== 'confirm') return null
  return {
    what: 'Couldn’t confirm it’s you.',
    next: confirm.step === 'password' ? 'Nothing was saved. Check the password, then try again.' : 'Nothing was saved. Try again.',
    detail: daemonErrorText(e),
  }
}

/** The password step of the owner confirm (a Mac confirms on its own). */
function ConfirmStep({ confirm, onSubmit }: { confirm: OwnerConfirm; onSubmit: () => void }): React.JSX.Element | null {
  const id = useId()
  if (confirm.step === 'mac') {
    return (
      <p className={`${DIALOG_MUTED} flex items-center gap-2`} role="status">
        <Spinner />
        Confirm on this Mac with Touch ID or your login password…
      </p>
    )
  }
  if (confirm.step !== 'password') return null
  return (
    <Field
      label={confirm.pwMethod === 'connect-password' ? 'Your login password' : 'Your vault password'}
      htmlFor={id}
      hint="K2 asks because this key can hire agents."
    >
      <Input
        id={id}
        type="password"
        autoFocus
        autoComplete="current-password"
        aria-label="Confirm password"
        className={DIALOG_FIELD}
        value={confirm.password}
        onChange={(e) => confirm.setPassword(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            onSubmit()
          }
        }}
      />
    </Field>
  )
}

/** One choice with its label and a line under it (a checkbox or radio). */
function Choice({
  control,
  label,
  hint,
}: {
  control: React.ReactNode
  label: React.ReactNode
  hint?: React.ReactNode
}): React.JSX.Element {
  return (
    <label className="flex items-start gap-2.5 px-3 py-2 cursor-pointer transition-colors hover:bg-[var(--color-wash-1)]">
      <span className="mt-0.5 flex">{control}</span>
      <span className="min-w-0">
        <span className={`block ${SETTING_ROW_LABEL}`}>{label}</span>
        {hint ? <span className={`block ${DIALOG_MUTED}`}>{hint}</span> : null}
      </span>
    </label>
  )
}

const ERROR_LINE = 'type-hint text-[var(--color-status-error-soft)] mt-1'

/**
 * "App access for hired agents": the most an App login made for a hired
 * agent may get — one App, its rooms, and the highest role. "No App
 * access" (the empty choice) means hired agents get no App login.
 */
function AppAccessLimit({
  form,
  onChange,
  opts,
  optsError,
}: {
  form: CeilingForm
  onChange: (f: CeilingForm) => void
  opts: CeilingOptions
  optsError: string | null
}): React.JSX.Element {
  const app = opts.apps.find((a) => a.id === form.app)
  const appRooms = app ? app.roomIds : []
  const roomRows = useMemo(() => {
    const self = opts.rooms.filter((r) => r.id === SELF_ROOM)
    const used = opts.rooms.filter((r) => r.id !== SELF_ROOM && appRooms.includes(r.id))
    const other = opts.rooms.filter((r) => r.id !== SELF_ROOM && !appRooms.includes(r.id))
    // A stored room this list no longer has stays visible (and removable).
    const gone = form.rooms
      .filter((id) => !opts.rooms.some((r) => r.id === id))
      .map((id) => ({ id, handle: id, name: 'No longer on this server' }))
    return { first: [...self, ...used, ...gone], other }
  }, [opts.rooms, appRooms, form.rooms])

  const toggleRoom = (id: string, on: boolean) => {
    const rooms = on ? [...form.rooms.filter((r) => r !== id), id] : form.rooms.filter((r) => r !== id)
    onChange({ ...form, rooms })
  }
  const firstRooms = useShowAll(roomRows.first, (r) => form.rooms.includes(r.id))
  const otherRooms = useShowAll(roomRows.other, (r) => form.rooms.includes(r.id))
  const roomBox = (r: CeilingRoom) => (
    <Choice
      key={r.id}
      control={
        <Checkbox checked={form.rooms.includes(r.id)} onChange={(e) => toggleRoom(r.id, e.target.checked)} aria-label={`Room ${r.handle || r.id}`} />
      }
      label={r.id === SELF_ROOM ? r.name : r.handle || r.name}
      hint={r.id !== SELF_ROOM && r.name && r.name !== r.handle ? r.name : undefined}
    />
  )

  return (
    <div className="space-y-3" data-testid="app-access-limit">
      <div>
        <p className={DIALOG_LABEL}>App access for hired agents</p>
        <p className={DIALOG_MUTED}>The most an App login made for one of its agents may get: one App, some of its rooms, and a highest role.</p>
      </div>
      {optsError ? (
        <Callout tone="error" role="alert" className="type-hint space-y-0.5">
          <p className="font-medium">Couldn’t load the Apps on this server.</p>
          <p>You can still choose No App access. The server said: {optsError.trim().replace(/[.\s]+$/, '')}.</p>
        </Callout>
      ) : null}
      <div>
        <p className={DIALOG_LABEL}>App</p>
        <div>
          <SettingDropdown
            fullWidth
            menuAlign="left"
            ariaLabel="App"
            value={form.app}
            options={[
              { value: '', label: NO_APP_ACCESS },
              ...opts.apps.map((a) => ({ value: a.id, label: a.workspace ? `${a.name} (${a.workspace})` : a.name })),
              ...(form.app && !app ? [{ value: form.app, label: `${form.app} (not published any more)` }] : []),
            ]}
            onChange={(v) => {
              // A re-pick of the same App keeps the rooms and role.
              if (v === form.app) return
              onChange({ app: v, rooms: v ? [SELF_ROOM] : [], role: '' })
            }}
          />
        </div>
        {!form.app ? (
          <p className={`${DIALOG_MUTED} mt-1`} data-testid="app-access-none">
            {NO_APP_ACCESS}: agents hired with this key get no App login.
            {opts.apps.length === 0 && !optsError ? ' No Apps are published on this server yet.' : ''}
          </p>
        ) : null}
      </div>
      {form.app ? (
        <>
          <div>
            <p className={DIALOG_LABEL}>Rooms</p>
            <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]" data-testid="app-access-rooms">
              {firstRooms.shown.map(roomBox)}
              <ShowAllRow list={firstRooms} total={roomRows.first.length} noun="rooms" />
              {roomRows.other.length > 0 ? <p className={`px-3 py-2 ${DIALOG_MUTED}`}>Other rooms on this server</p> : null}
              {otherRooms.shown.map(roomBox)}
              <ShowAllRow list={otherRooms} total={roomRows.other.length} noun="other rooms" />
            </Surface>
            {form.rooms.length === 0 ? (
              <p className={ERROR_LINE} data-field-problem="">
                Pick at least one room, or choose {NO_APP_ACCESS}.
              </p>
            ) : null}
          </div>
          <div>
            <p className={DIALOG_LABEL}>Highest role</p>
            <div>
              <SettingDropdown
                fullWidth
                menuAlign="left"
                ariaLabel="Highest role"
                value={form.role}
                options={[
                  { value: '', label: 'No role (Thread only in these rooms)' },
                  ...(app?.roles ?? []).map((r) => ({ value: r.id, label: r.appId ? r.name : `${r.name} (all Apps)` })),
                  ...(form.role && !app?.roles.some((r) => r.id === form.role)
                    ? [{ value: form.role, label: `${form.role} (not found)` }]
                    : []),
                ]}
                onChange={(v) => onChange({ ...form, role: v })}
              />
            </div>
            <p className={`${DIALOG_MUTED} mt-1`}>A hired agent may get this role, or a lower one, in the rooms ticked above.</p>
          </div>
        </>
      ) : null}
      <p className={DIALOG_MUTED} data-testid="hire-whole-room-warning">
        An App login for a hired agent opens whole rooms, not single folders.
      </p>
    </div>
  )
}

/** A whole number of at least 1, or the problem with it. */
const countProblem = (raw: string): string | null => {
  const n = Number(raw.trim())
  return Number.isInteger(n) && n >= 1 ? null : 'Use a whole number, 1 or more.'
}

const REACH_OPTIONS: Array<{ value: HireForm['reach']; label: string }> = [
  { value: 'none', label: 'Only hire them' },
  { value: 'message', label: 'Message them' },
  { value: 'host-sessions', label: 'Message them and run their sessions' },
]

/** What the done step says the new key reaches. */
export interface CreatedKey {
  id: string | null
  name: string
  secret: string
}

/**
 * Create API key: a name, where it reaches, what it can do (and, for a key
 * that hires agents, its limits). Then the key, once, with Copy and Done.
 */
export function CreateKeyDialog({
  workspaces,
  ceilingOpts,
  ceilingOptsError,
  apiOff,
  onCreated,
  onClose,
}: {
  workspaces: Array<{ slug: string; label: string }>
  ceilingOpts: CeilingOptions
  ceilingOptsError: string | null
  /** The public API is off: the done step says the key won't work yet. */
  apiOff: boolean
  /** The key exists (re-read the list). */
  onCreated: () => void
  /** Closed: with the new key's name when one was made. */
  onClose: (created: CreatedKey | null) => void
}): React.JSX.Element {
  const ids = { name: useId(), folder: useId(), day: useId(), live: useId(), team: useId(), form: useId() }
  const formRef = useRef<HTMLFormElement>(null)
  const [name, setName] = useState('')
  const [wsMode, setWsMode] = useState<'pick' | 'all'>('pick')
  const [picked, setPicked] = useState<Set<string>>(new Set())
  const [doors, setDoors] = useState<Record<KeyDoor, boolean>>({ hostSessions: true, canonicalMessage: false, sandboxes: false, db: false })
  const [hire, setHire] = useState(false)
  const [hireForm, setHireForm] = useState<HireForm>(HIRE_FORM_DEFAULTS)
  const [ceiling, setCeiling] = useState<CeilingForm>(EMPTY_CEILING_FORM)
  const [tried, setTried] = useState(false)
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<Problem | null>(null)
  const [created, setCreated] = useState<CreatedKey | null>(null)
  const confirm = useOwnerConfirm(finishHireKeyConfirm)
  const setHireField = <K extends keyof HireForm>(k: K, v: HireForm[K]) => setHireForm((prev) => ({ ...prev, [k]: v }))
  const setDoor = (k: KeyDoor, on: boolean) => {
    setDoors((prev) => ({ ...prev, [k]: on }))
    // Hired agents can't get more database access than the key has.
    if (k === 'db' && !on) setHireField('db', 'off')
  }

  const wsList = useShowAll(workspaces, (w) => picked.has(w.slug))
  const anyDoor = Object.values(doors).some(Boolean)
  const wsNeeded = KEY_DOORS.some((d) => d.needsWorkspace && doors[d.key])
  const problems = {
    ws: wsNeeded && wsMode === 'pick' && picked.size === 0 ? 'Pick at least one workspace, or choose every workspace.' : null,
    doors: !anyDoor && !hire ? 'Choose at least one thing this key can do.' : null,
    day: hire ? countProblem(hireForm.maxMembersPerDay) : null,
    live: hire ? countProblem(hireForm.maxLiveMembers) : null,
    team: hire ? countProblem(hireForm.maxMembersPerTeam) : null,
    rooms: hire && ceiling.app && ceiling.rooms.length === 0 ? 'rooms' : null,
  }
  const blocked = Object.values(problems).some(Boolean)
  const show = (p: string | null): string | null => (tried ? p : null)

  const mint = async (confirmToken?: string): Promise<void> => {
    const body: Record<string, unknown> = {
      label: name.trim() || undefined,
      capabilities: { ...doors },
    }
    if (wsMode === 'all') body.workspaces = '*'
    else if (picked.size > 0) body.workspaces = [...picked]
    if (hire) {
      const h = hireLimitsFromForm(hireForm, doors.db)
      const apps = ceilingFromForm(ceiling, ceilingOpts)
      body.hire = apps.length ? { ...h, apps } : h
      body.confirmToken = confirmToken
    }
    const res = await createApiKey(body)
    if (!res.key) throw new Error(res.error || 'it answered without a key')
    setCreated({ id: res.id ?? null, name: name.trim() || 'Unnamed key', secret: res.key })
    onCreated()
  }

  const fail = (e: unknown): void => {
    setProblem(
      confirmProblem(confirm, e) ?? {
        what: 'Couldn’t create the key.',
        next: 'Nothing was created. Check your choices, then try again.',
        detail: daemonErrorText(e),
      },
    )
  }

  const submit = async (): Promise<void> => {
    if (busy) return
    setTried(true)
    if (blocked) {
      // After the marks render: bring the first one into view.
      requestAnimationFrame(() => scrollToFirstProblem(formRef.current))
      return
    }
    setBusy(true)
    setProblem(null)
    try {
      if (confirm.step === 'password') await confirm.submitPassword()
      else if (!hire) await mint()
      // A key that can hire agents needs a fresh owner confirm (Touch ID /
      // login password, or the vault password), owner token included.
      else await confirm.begin(mint)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  if (created) {
    return (
      <AccessSheet
        title="Copy your new API key"
        onClose={() => onClose(created)}
        testId="api-key-created-dialog"
        footer={
          <Button variant="accent" size="md" onClick={() => onClose(created)} autoFocus>
            Done
          </Button>
        }
      >
        <p className={DIALOG_MUTED}>
          {created.name} is ready. This is the only time K2 shows its key: copy it now and paste it into the app that will
          use it. If it’s ever lost, revoke it and create another.
        </p>
        <Surface chamfer material="tile">
          <CopyLine label="API key" value={created.secret} testId="one-time-secret" />
        </Surface>
        {apiOff ? (
          <Callout tone="warn" className="type-hint">
            The public API is off on this server, so this key won’t work until it’s turned on.
          </Callout>
        ) : null}
      </AccessSheet>
    )
  }

  const numberField = (key: 'maxMembersPerDay' | 'maxLiveMembers' | 'maxMembersPerTeam', id: string, label: string, p: string | null) => (
    <div>
      <label htmlFor={id} className={DIALOG_LABEL}>
        {label}
      </label>
      <Input
        id={id}
        inputMode="numeric"
        aria-label={label}
        aria-invalid={p ? true : undefined}
        className={DIALOG_FIELD}
        value={hireForm[key]}
        onChange={(e) => setHireField(key, e.target.value)}
      />
      {p ? (
        <p className={ERROR_LINE} data-field-problem="">
          {p}
        </p>
      ) : null}
    </div>
  )

  return (
    <AccessSheet
      title="Create API key"
      intro="An app or service uses the key to reach this server. You choose which workspaces it reaches and what it can do there."
      onClose={() => onClose(null)}
      busy={busy}
      testId="create-api-key-dialog"
      footer={
        <>
          {tried && blocked && !problem ? (
            <p className="mr-auto min-w-0 self-center type-hint text-[var(--color-status-error-soft)]" role="alert">
              Fix the marked choices above, then create the key.
            </p>
          ) : null}
          <Button variant="ghost" size="md" onClick={() => onClose(null)} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={ids.form} variant="accent" size="md" disabled={busy || (confirm.step === 'password' && !confirm.password)}>
            {busy ? 'Creating…' : confirm.step === 'password' ? 'Confirm and create' : 'Create key'}
          </Button>
        </>
      }
    >
      <form
        id={ids.form}
        ref={formRef}
        className="space-y-5"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <Field label="Name" htmlFor={ids.name} hint="So you can tell your keys apart, like the app that will use it.">
          <Input id={ids.name} autoFocus className={DIALOG_FIELD} value={name} maxLength={80} onChange={(e) => setName(e.target.value)} />
        </Field>

        <fieldset className="space-y-2">
          <legend className={DIALOG_LABEL}>What it can do</legend>
          <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]">
            {KEY_DOORS.map((d) => (
              <Choice
                key={d.key}
                control={<Checkbox aria-label={d.label} checked={doors[d.key]} onChange={(e) => setDoor(d.key, e.target.checked)} />}
                label={d.label}
                hint={d.hint}
              />
            ))}
            <Choice
              control={<Checkbox aria-label="Hire agents" checked={hire} onChange={(e) => setHire(e.target.checked)} />}
              label="Hire agents"
              hint="Add agents to this server, within limits you set below."
            />
          </Surface>
          {show(problems.doors) ? (
            <p className={ERROR_LINE} data-field-problem="">
              {problems.doors}
            </p>
          ) : null}
        </fieldset>

        <fieldset className="space-y-2">
          <legend className={DIALOG_LABEL}>Workspaces it can reach</legend>
          <p className={`${DIALOG_MUTED} -mt-1`}>
            {wsNeeded ? 'Agent sessions, messages and databases work only in these.' : 'Not needed for sandboxes or hiring agents.'}
          </p>
          <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]" role="radiogroup" aria-label="Workspaces it can reach">
            <Choice
              control={<Radio name="api-key-workspaces" checked={wsMode === 'pick'} onChange={() => setWsMode('pick')} aria-label="Only the ones I pick" />}
              label="Only the ones I pick"
            />
            {wsMode === 'pick' ? (
              <div className="pl-7 divide-y divide-[var(--color-border)]" data-testid="api-key-workspace-list">
                {workspaces.length === 0 ? (
                  <p className={`px-3 py-2.5 ${DIALOG_MUTED}`}>There are no workspaces on this server yet.</p>
                ) : (
                  wsList.shown.map(({ slug, label }) => (
                    <Choice
                      key={slug}
                      control={
                        <Checkbox
                          aria-label={label}
                          checked={picked.has(slug)}
                          onChange={(e) =>
                            setPicked((prev) => {
                              const next = new Set(prev)
                              if (e.target.checked) next.add(slug)
                              else next.delete(slug)
                              return next
                            })
                          }
                        />
                      }
                      label={label}
                    />
                  ))
                )}
                <ShowAllRow list={wsList} total={workspaces.length} noun="workspaces" />
              </div>
            ) : null}
            <Choice
              control={<Radio name="api-key-workspaces" checked={wsMode === 'all'} onChange={() => setWsMode('all')} aria-label="Every workspace, including new ones" />}
              label="Every workspace, including new ones"
            />
          </Surface>
          {show(problems.ws) ? (
            <p className={ERROR_LINE} data-field-problem="">
              {problems.ws}
            </p>
          ) : null}
        </fieldset>

        {hire ? (
          <fieldset className="space-y-4" data-testid="hire-scope" aria-label="Limits for hired agents">
            <div>
              <p className={DIALOG_LABEL}>Limits for hired agents</p>
              <p className={DIALOG_MUTED}>
                Agents it hires never get more than this key allows. Later you can change only their App access; for anything else,
                create a new key. {hireConfirmCopy()}
              </p>
            </div>
            <div className="grid grid-cols-3 gap-3">
              {numberField('maxMembersPerDay', ids.day, 'Agents per day', show(problems.day))}
              {numberField('maxLiveMembers', ids.live, 'Live agents', show(problems.live))}
              {numberField('maxMembersPerTeam', ids.team, 'Agents per team', show(problems.team))}
            </div>
            <div>
              <p className={DIALOG_LABEL}>What it can do with its agents</p>
              <div>
                <SettingDropdown
                  fullWidth
                  menuAlign="left"
                  ariaLabel="What it can do with its agents"
                  value={hireForm.reach}
                  options={REACH_OPTIONS}
                  onChange={(v) => setHireField('reach', v as HireForm['reach'])}
                />
              </div>
            </div>
            <div>
              <p className={DIALOG_LABEL}>Database access for hired agents</p>
              <div>
                <SettingDropdown
                  fullWidth
                  menuAlign="left"
                  ariaLabel="Database access for hired agents"
                  value={hireForm.db}
                  options={[
                    { value: 'off', label: 'None' },
                    { value: 'read', label: 'Read only', disabled: !doors.db },
                    { value: 'write', label: 'Read and change', disabled: !doors.db },
                  ]}
                  onChange={(v) => setHireField('db', v as HireForm['db'])}
                />
              </div>
              {!doors.db ? <p className={`${DIALOG_MUTED} mt-1`}>Needs Workspace databases above.</p> : null}
            </div>
            <Field
              label="Folder for their workspaces"
              htmlFor={ids.folder}
              hint="A folder inside the hire folder. Leave it empty to use the hire folder itself."
            >
              <Input
                id={ids.folder}
                className={DIALOG_FIELD}
                value={hireForm.folder}
                onChange={(e) => setHireField('folder', e.target.value)}
              />
            </Field>
            <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]">
              <Choice
                control={<Checkbox aria-label="Link its agents into teams" checked={hireForm.link} onChange={(e) => setHireField('link', e.target.checked)} />}
                label="Link its agents into teams"
                hint="Only agents this key hired."
              />
              <Choice
                control={<Checkbox aria-label="Retire agents it hired" checked={hireForm.retire} onChange={(e) => setHireField('retire', e.target.checked)} />}
                label="Retire agents it hired"
              />
            </Surface>
            <AppAccessLimit form={ceiling} onChange={setCeiling} opts={ceilingOpts} optsError={ceilingOptsError} />
          </fieldset>
        ) : null}

        <ConfirmStep confirm={confirm} onSubmit={() => void submit()} />
        {problem ? <ProblemNote problem={problem} /> : null}
      </form>
    </AccessSheet>
  )
}

/**
 * Change a hire key's App access. The daemon decides: lowering (fewer
 * rooms, a lower role, no App access) saves straight away; widening answers
 * `vault_confirm_required`, and this asks for the same owner confirm as
 * creating the key, then saves with the token.
 */
export function AppAccessDialog({
  keyRow,
  ceilingOpts,
  ceilingOptsError,
  onClose,
  onSaved,
}: {
  keyRow: ApiKeyRow
  ceilingOpts: CeilingOptions
  ceilingOptsError: string | null
  onClose: () => void
  onSaved: () => void
}): React.JSX.Element {
  const stored = keyRow.hireLimits?.apps ?? []
  const [form, setForm] = useState<CeilingForm>(() => ceilingFormFromLimits(stored))
  const confirm = useOwnerConfirm(finishCeilingConfirm)
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<Problem | null>(null)
  const roomsMissing = !!form.app && form.rooms.length === 0
  const formId = useId()

  const save = async (): Promise<void> => {
    if (busy || roomsMissing) return
    setBusy(true)
    setProblem(null)
    try {
      if (confirm.step === 'password') {
        await confirm.submitPassword()
        return
      }
      const apps = ceilingFromForm(form, ceilingOpts)
      try {
        await saveHireCeiling(keyRow.id, apps)
        onSaved()
      } catch (e) {
        if (daemonErrorOf(e)?.code !== 'vault_confirm_required') throw e
        // Widening: confirm, then save again with the one-use token.
        await confirm.begin(async (token) => {
          await saveHireCeiling(keyRow.id, apps, token)
          onSaved()
        })
      }
    } catch (e) {
      setProblem(
        confirmProblem(confirm, e) ?? {
          what: 'Couldn’t save App access.',
          next: 'It’s unchanged. Try again in a moment.',
          detail: daemonErrorText(e),
        },
      )
    } finally {
      setBusy(false)
    }
  }

  return (
    <AccessSheet
      title={`App access for ${keyName(keyRow)}`}
      intro="Which App logins the agents this key hires may get. Lowering it saves right away; widening it asks you to confirm it’s you. Logins already made keep what they have."
      onClose={onClose}
      busy={busy}
      testId="edit-app-access"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={formId} variant="accent" size="md" disabled={busy || roomsMissing || (confirm.step === 'password' && !confirm.password)}>
            {busy ? 'Saving…' : confirm.step === 'password' ? 'Confirm and save' : 'Save'}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        className="space-y-5"
        onSubmit={(e) => {
          e.preventDefault()
          void save()
        }}
      >
        <p className={DIALOG_MUTED}>
          <span className="text-[var(--color-text-primary)]">Now:</span> {ceilingSummary(stored, ceilingOpts)}
        </p>
        {stored.length > 1 ? (
          <Callout tone="warn" className="type-hint">
            This key names {stored.length} Apps. Saving here keeps only the one shown.
          </Callout>
        ) : null}
        <AppAccessLimit form={form} onChange={setForm} opts={ceilingOpts} optsError={ceilingOptsError} />
        <ConfirmStep confirm={confirm} onSubmit={() => void save()} />
        {problem ? <ProblemNote problem={problem} /> : null}
      </form>
    </AccessSheet>
  )
}
