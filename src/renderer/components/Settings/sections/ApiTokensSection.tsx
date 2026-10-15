// Settings → K2 API Tokens — daemon-wide inventory of k2sk_ keys.
// Owner-tier routes (/cli/api-keys/*). Secret shown once on create only.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import type { SettingEntry } from '../searchManifest'
import {
  capsSummary,
  ceilingFormFromLimits,
  ceilingFromForm,
  ceilingSummary,
  EMPTY_CEILING_FORM,
  fetchCeilingOptions,
  finishCeilingConfirm,
  finishHireKeyConfirm,
  formatGrant,
  HIRE_FORM_DEFAULTS,
  hireLimitsFromForm,
  hireSummary,
  keyGrantsWorkspace,
  keyState,
  NO_APP_ACCESS,
  saveHireCeiling,
  SELF_ROOM,
  startHireKeyConfirm,
  type ApiKeyRow,
  type CeilingForm,
  type CeilingOptions,
  type CeilingRoom,
  type HireForm,
} from './api-keys-api'
import { primaryScope } from '@/kessel/server-scope'
import { SquareCheckbox } from '@/components/ui'
import { currentShortcutOs } from '@/lib/shortcut-label'
import { isAirgap } from '@/lib/airgap'
import { useDialogEscape } from '../settings-esc'
import type { DesktopOs } from '@/lib/desktop-chrome'
import { AccessAuditLink } from '../shared/AccessAuditLink'

/** Where the "Enable public API (/v1)" toggle lives: the Policies column
 *  of Network → Tunnel, or of Connected Servers under air-gap (Tunnel is
 *  hidden then; `K2ConnectSettingsShell`). */
export function publicApiToggleLocation(airgap: boolean = isAirgap()): string {
  return airgap
    ? 'Settings → Network → Connected Servers → Policies'
    : 'Settings → Network → Tunnel → Policies'
}

/** The hire-key confirm copy. Touch ID (`local-auth`) exists only for the
 *  owner on a Mac; Linux and Windows confirm with a password (the Connect
 *  login password or the vault password, `vault_routes.rs` `methods`). */
export function hireConfirmCopy(os: DesktopOs = currentShortcutOs()): string {
  return os === 'mac'
    ? 'Creating it asks you to confirm (Touch ID or a password).'
    : 'Creating it asks for your password to confirm.'
}

export const API_TOKENS_MANIFEST: SettingEntry[] = [
  {
    id: 'api-tokens.inventory',
    section: 'api-tokens',
    label: 'API Keys',
    description: 'Mint, disable, and revoke top-level k2sk_ keys for the public /v1 API',
    keywords: [
      'api',
      'api key',
      'k2sk',
      'token',
      'host-sessions',
      'revoke',
      'disable',
      'public api',
      'v1',
      'hire',
      'hire agents',
      'database',
    ],
  },
]

type ListResponse = { keys?: ApiKeyRow[] }

function stateBadge(state: ReturnType<typeof keyState>): React.JSX.Element {
  const cls =
    state === 'active'
      ? 'text-[var(--color-status-success-soft)]'
      : state === 'disabled'
        ? 'text-[var(--color-status-warning-soft)]'
        : 'text-[var(--color-text-muted)]'
  return <span className={`text-[10px] font-semibold uppercase ${cls}`}>{state}</span>
}

export function ApiTokensSection(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const apiEnabled = useSettingsStore((s) => s.apiEnabled)
  const [keys, setKeys] = useState<ApiKeyRow[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [createOpen, setCreateOpen] = useState(false)
  const [mintedSecret, setMintedSecret] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [ceilingOpts, setCeilingOpts] = useState<CeilingOptions>({ apps: [], rooms: [] })
  const [ceilingOptsError, setCeilingOptsError] = useState<string | null>(null)
  const [editing, setEditing] = useState<ApiKeyRow | null>(null)

  const refresh = useCallback(async () => {
    setError(null)
    try {
      const d = await daemonCliGet<ListResponse>(primaryScope(), 'api-keys/list')
      setKeys(Array.isArray(d.keys) ? d.keys : [])
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setKeys([])
    } finally {
      setLoading(false)
    }
    try {
      setCeilingOpts(await fetchCeilingOptions())
      setCeilingOptsError(null)
    } catch (e) {
      setCeilingOptsError(daemonErrorText(e))
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const act = useCallback(
    async (id: string, action: 'disable' | 'enable' | 'revoke') => {
      if (action === 'revoke') {
        const ok = window.confirm(
          'Permanently revoke this API key? It cannot be re-enabled — you must mint a new secret.',
        )
        if (!ok) return
      }
      setBusyId(id)
      setError(null)
      try {
        await daemonCliPost(primaryScope(), `api-keys/${action}`, { id })
        await refresh()
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusyId(null)
      }
    },
    [refresh],
  )

  return (
    <div className="max-w-4xl space-y-6">
      <div>
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">API Keys</h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1 max-w-2xl">
          Top-level <code className="text-[10px]">k2sk_…</code> keys for the public{' '}
          <code className="text-[10px]">/v1</code> API (host-sessions, sandboxes, …). The raw secret
          is shown only once when created. Disable for emergencies (re-enableable); revoke is
          permanent.
        </p>
        <AccessAuditLink className="mt-1" />
        {!apiEnabled && (
          <p className="text-[11px] text-[var(--color-status-warning-soft)] mt-1 max-w-2xl" data-testid="api-surface-off">
            The public /v1 API is off on this server, so no key works yet (hiring agents included).
            Turn it on under {publicApiToggleLocation()} → Enable public API (/v1), or{' '}
            <code className="text-[10px]">k2 api on</code>.
          </p>
        )}
      </div>

      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => {
            setMintedSecret(null)
            setCreateOpen(true)
          }}
          className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer"
        >
          Create API key
        </button>
        <button
          type="button"
          onClick={() => void refresh()}
          className="px-3 py-1.5 text-[11px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)] cursor-pointer"
        >
          Refresh
        </button>
      </div>

      {error && (
        <p className="text-[11px] text-[var(--color-status-error-soft)] max-w-2xl">{error}</p>
      )}

      {mintedSecret && (
        <div className="border border-[var(--color-status-warning-soft)]/40 bg-[var(--color-status-warning-soft)]/10 p-3 space-y-2">
          <p className="text-[11px] font-semibold text-[var(--color-text-primary)]">
            Store this key now — it cannot be retrieved again
          </p>
          <code className="block text-[11px] break-all select-all text-[var(--color-text-primary)]">
            {mintedSecret}
          </code>
          <button
            type="button"
            className="text-[11px] text-[var(--color-accent)] cursor-pointer"
            onClick={() => {
              void navigator.clipboard?.writeText(mintedSecret)
            }}
          >
            Copy to clipboard
          </button>
        </div>
      )}

      {loading ? (
        <p className="text-[11px] text-[var(--color-text-muted)]">Loading keys…</p>
      ) : keys.length === 0 ? (
        <p className="text-[11px] text-[var(--color-text-muted)]">
          No API keys yet. Create one to allow external apps (e.g. Scout) to call host-sessions on
          granted workspaces.
        </p>
      ) : (
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          {keys.map((k) => {
            const state = keyState(k)
            const busy = busyId === k.id
            return (
              <div key={k.id} className="p-3 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
                <div className="min-w-0 space-y-1">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="text-[12px] font-medium text-[var(--color-text-primary)]">
                      {k.label || '(no label)'}
                    </span>
                    {stateBadge(state)}
                  </div>
                  <p className="text-[10px] font-mono text-[var(--color-text-muted)] break-all">
                    {k.id}
                  </p>
                  <p className="text-[11px] text-[var(--color-text-secondary)]">
                    Workspaces: {formatGrant(k.allowedWorkspaces)}
                  </p>
                  <p className="text-[11px] text-[var(--color-text-secondary)]">
                    Caps: {capsSummary(k.capabilities)}
                    {k.anthropicKeySet ? ' · LLM key staged' : ' · no LLM key'}
                    {k.provider ? ` · ${k.provider}` : ''}
                  </p>
                  {k.capabilities?.hire && k.hireLimits && (
                    <p className="text-[11px] text-[var(--color-text-secondary)]" data-testid={`hire-limits-${k.id}`}>
                      Hires agents: {hireSummary(k.hireLimits)}
                    </p>
                  )}
                  {k.capabilities?.hire && k.hireLimits && (
                    <p className="text-[11px] text-[var(--color-text-secondary)]" data-testid={`hire-ceiling-${k.id}`}>
                      App access: {ceilingSummary(k.hireLimits.apps, ceilingOpts)}
                    </p>
                  )}
                </div>
                <div className="flex flex-wrap gap-1.5 flex-shrink-0">
                  {state !== 'revoked' && k.capabilities?.hire && k.hireLimits && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => setEditing(k)}
                      aria-label={`Change App access for ${k.label || k.id}`}
                      className="px-2 py-1 text-[10px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-muted)] cursor-pointer disabled:opacity-50"
                    >
                      App access…
                    </button>
                  )}
                  {state === 'active' && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void act(k.id, 'disable')}
                      className="px-2 py-1 text-[10px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-status-warning-soft)] cursor-pointer disabled:opacity-50"
                    >
                      Disable
                    </button>
                  )}
                  {state === 'disabled' && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void act(k.id, 'enable')}
                      className="px-2 py-1 text-[10px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-status-success-soft)] cursor-pointer disabled:opacity-50"
                    >
                      Enable
                    </button>
                  )}
                  {state !== 'revoked' && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void act(k.id, 'revoke')}
                      className="px-2 py-1 text-[10px] border border-[var(--color-border)] text-[var(--color-status-error-soft)] hover:border-[var(--color-status-error-soft)] cursor-pointer disabled:opacity-50"
                    >
                      Revoke
                    </button>
                  )}
                </div>
              </div>
            )
          })}
        </div>
      )}

      {createOpen && (
        <CreateKeyModal
          workspaceOptions={projects.map((p) => ({
            id: p.id,
            name: p.name,
            path: p.path,
          }))}
          ceilingOpts={ceilingOpts}
          ceilingOptsError={ceilingOptsError}
          onClose={() => setCreateOpen(false)}
          onCreated={(secret) => {
            setCreateOpen(false)
            setMintedSecret(secret)
            void refresh()
          }}
        />
      )}

      {editing && (
        <EditCeilingModal
          keyRow={editing}
          ceilingOpts={ceilingOpts}
          ceilingOptsError={ceilingOptsError}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null)
            void refresh()
          }}
        />
      )}
    </div>
  )
}

/**
 * The owner confirm (Touch ID / login password, or the vault password) for
 * a hire-key mint or an App access raise. `begin(then)` runs `then(token)`
 * right away on a Mac, or after the password prompt.
 */
function useOwnerConfirm(finish: (method: string, password?: string) => Promise<{ token: string }>) {
  const [step, setStep] = useState<'idle' | 'mac' | 'password'>('idle')
  const [pwMethod, setPwMethod] = useState('vault-password')
  const pwRef = useRef<HTMLInputElement>(null)
  const pending = useRef<((token: string) => Promise<void>) | null>(null)

  const begin = async (then: (token: string) => Promise<void>): Promise<void> => {
    try {
      const { methods } = await startHireKeyConfirm()
      if (methods.includes('local-auth')) {
        setStep('mac')
        const { token } = await finish('local-auth')
        setStep('idle')
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
    throw new Error('No way to confirm here yet. Set a vault password from a terminal outside K2: k2 vault confirm-password set.')
  }

  const submitPassword = async (): Promise<void> => {
    const el = pwRef.current
    if (!el) return
    const pw = el.value
    el.value = ''
    const { token } = await finish(pwMethod, pw)
    setStep('idle')
    const then = pending.current
    pending.current = null
    if (then) await then(token)
  }

  return { step, pwMethod, pwRef, begin, submitPassword }
}

function ConfirmPrompt({
  confirm,
  submitting,
  onSubmit,
  buttonLabel,
}: {
  confirm: ReturnType<typeof useOwnerConfirm>
  submitting: boolean
  onSubmit: () => void
  buttonLabel: string
}): React.JSX.Element | null {
  if (confirm.step === 'mac') {
    return <p className="text-[11px] text-[var(--color-text-secondary)]">Confirm on this Mac (Touch ID or your login password)…</p>
  }
  if (confirm.step !== 'password') return null
  return (
    <div className="space-y-1">
      <p className="text-[11px] text-[var(--color-text-secondary)]">
        {confirm.pwMethod === 'connect-password' ? 'Your login password' : 'Vault password'} to confirm
      </p>
      <input
        ref={confirm.pwRef}
        type="password"
        aria-label="Confirm password"
        className="w-full px-2 py-1.5 text-[12px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
        onKeyDown={(e) => {
          if (e.key === 'Enter') onSubmit()
        }}
      />
      <button
        type="button"
        disabled={submitting}
        onClick={onSubmit}
        className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] cursor-pointer disabled:opacity-50"
      >
        {buttonLabel}
      </button>
    </div>
  )
}

/**
 * "App access limit": the most an App login made for a hired agent may
 * get — one App, its rooms, and the highest role. "No App access" (the
 * empty choice) means hired agents get no App login.
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
      .map((id) => ({ id, handle: id, name: '(no longer on this server)' }))
    return { first: [...self, ...used, ...gone], other }
  }, [opts.rooms, appRooms, form.rooms])

  const toggleRoom = (id: string, on: boolean) => {
    const rooms = on ? [...form.rooms.filter((r) => r !== id), id] : form.rooms.filter((r) => r !== id)
    onChange({ ...form, rooms })
  }
  const roomBox = (r: CeilingRoom) => (
    <label key={r.id} className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer">
      <SquareCheckbox
        checked={form.rooms.includes(r.id)}
        onChange={(e) => toggleRoom(r.id, e.target.checked)}
        aria-label={`Room ${r.handle || r.id}`}
      />
      <span className="truncate">{r.id === SELF_ROOM ? r.name : r.handle || r.name}</span>
      {r.id !== SELF_ROOM && r.name && r.name !== r.handle && (
        <span className="text-[10px] text-[var(--color-text-muted)] truncate">{r.name}</span>
      )}
    </label>
  )

  return (
    <div className="space-y-1.5 border-t border-[var(--color-border)] pt-1.5" data-testid="app-access-limit">
      <p className="text-[11px] text-[var(--color-text-secondary)]">App access limit</p>
      <p className="text-[10px] text-[var(--color-text-muted)]">
        The most an App login made for a hired agent may get: one App, its rooms, and the highest role.
      </p>
      {optsError && <p className="text-[10px] text-[var(--color-status-error-soft)]">Couldn&apos;t load Apps: {optsError}</p>}
      <label className="flex items-center justify-between gap-2 text-[11px] text-[var(--color-text-secondary)]">
        App
        <select
          aria-label="App"
          value={form.app}
          onChange={(e) => onChange({ app: e.target.value, rooms: e.target.value ? [SELF_ROOM] : [], role: '' })}
          className="max-w-[60%] px-2 py-1 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
        >
          <option value="">{NO_APP_ACCESS}</option>
          {opts.apps.map((a) => (
            <option key={a.id} value={a.id}>
              {a.workspace ? `${a.name} (${a.workspace})` : a.name}
            </option>
          ))}
          {form.app && !app && <option value={form.app}>{form.app} (not published any more)</option>}
        </select>
      </label>
      {!form.app ? (
        <p className="text-[10px] text-[var(--color-text-muted)]" data-testid="app-access-none">
          {NO_APP_ACCESS}: agents hired with this key get no App login.
          {opts.apps.length === 0 && ' No Apps are published on this server yet.'}
        </p>
      ) : (
        <>
          <p className="text-[10px] text-[var(--color-text-muted)]">Rooms</p>
          <div className="max-h-32 overflow-y-auto border border-[var(--color-border)] p-2 space-y-1">
            {roomRows.first.map(roomBox)}
            {roomRows.other.length > 0 && (
              <p className="text-[10px] text-[var(--color-text-muted)] pt-1">Other rooms on this server</p>
            )}
            {roomRows.other.map(roomBox)}
          </div>
          {form.rooms.length === 0 && (
            <p className="text-[10px] text-[var(--color-status-warning-soft)]">Pick at least one room.</p>
          )}
          <label className="flex items-center justify-between gap-2 text-[11px] text-[var(--color-text-secondary)]">
            Highest role
            <select
              aria-label="Highest role"
              value={form.role}
              onChange={(e) => onChange({ ...form, role: e.target.value })}
              className="max-w-[60%] px-2 py-1 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
            >
              <option value="">No role (Thread only in these rooms)</option>
              {(app?.roles ?? []).map((r) => (
                <option key={r.id} value={r.id}>
                  {r.appId ? r.name : `${r.name} (all Apps)`}
                </option>
              ))}
              {form.role && !app?.roles.some((r) => r.id === form.role) && (
                <option value={form.role}>{form.role} (not found)</option>
              )}
            </select>
          </label>
          <p className="text-[10px] text-[var(--color-text-muted)]">
            A hired agent may get this role, or one no wider, when all its rooms are ticked above.
          </p>
        </>
      )}
      <p className="text-[10px] text-[var(--color-status-warning-soft)]" data-testid="hire-whole-room-warning">
        App logins for hired agents cover whole rooms until folder grants exist.
      </p>
    </div>
  )
}

function CreateKeyModal({
  workspaceOptions,
  ceilingOpts,
  ceilingOptsError,
  onClose,
  onCreated,
}: {
  workspaceOptions: Array<{ id: string; name: string; path: string }>
  ceilingOpts: CeilingOptions
  ceilingOptsError: string | null
  onClose: () => void
  onCreated: (secret: string) => void
}): React.JSX.Element {
  const [label, setLabel] = useState('')
  const [allWs, setAllWs] = useState(false)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [hostSessions, setHostSessions] = useState(true)
  const [canonicalMessage, setCanonicalMessage] = useState(false)
  const [sandboxes, setSandboxes] = useState(false)
  const [db, setDb] = useState(false)
  const [hire, setHire] = useState(false)
  const [hireForm, setHireForm] = useState<HireForm>(HIRE_FORM_DEFAULTS)
  const [ceiling, setCeiling] = useState<CeilingForm>(EMPTY_CEILING_FORM)
  const confirm = useOwnerConfirm(finishHireKeyConfirm)
  const [submitting, setSubmitting] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  useDialogEscape(onClose, !submitting)
  const setHireField = <K extends keyof HireForm>(k: K, v: HireForm[K]) =>
    setHireForm((prev) => ({ ...prev, [k]: v }))

  const slugs = useMemo(
    () =>
      workspaceOptions.map((p) => ({
        slug: (p.name || p.path.split(/[/\\]/).pop() || p.id).trim(),
        label: p.name || p.path,
      })),
    [workspaceOptions],
  )

  const hireBody = (): Record<string, unknown> => {
    const h = hireLimitsFromForm(hireForm, db)
    const apps = ceilingFromForm(ceiling, ceilingOpts)
    return apps.length ? { ...h, apps } : h
  }

  const mint = async (confirmToken?: string) => {
    const body: Record<string, unknown> = {
      label: label.trim() || undefined,
      capabilities: {
        hostSessions,
        canonicalMessage,
        sandboxes,
        db,
      },
    }
    if (allWs) {
      body.workspaces = '*'
    } else if (selected.size > 0) {
      body.workspaces = [...selected]
    }
    if (hire) {
      body.hire = hireBody()
      body.confirmToken = confirmToken
    }
    const res = await daemonCliPost<{ id?: string; key?: string; error?: string }>(primaryScope(),
      'api-keys/create',
      body,
    )
    if (!res.key) throw new Error(res.error || 'create returned no key')
    onCreated(res.key)
  }

  const submit = async () => {
    setSubmitting(true)
    setErr(null)
    try {
      if (!hire) {
        await mint()
        return
      }
      // Validate the limits before asking for the confirm.
      hireBody()
      // A key that can hire agents needs a fresh owner confirm (Touch ID /
      // login password, or the vault password), owner token included.
      await confirm.begin(mint)
    } catch (e) {
      setErr(daemonErrorText(e))
    } finally {
      setSubmitting(false)
    }
  }

  const submitPassword = async () => {
    setSubmitting(true)
    setErr(null)
    try {
      await confirm.submitPassword()
    } catch (e) {
      setErr(daemonErrorText(e))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/50 p-4" role="dialog" aria-modal="true" aria-label="Create API key">
      <div className="w-full max-w-md max-h-full overflow-y-auto border border-[var(--color-border)] bg-[var(--color-bg)] p-4 space-y-3 shadow-xl">
        <h3 className="text-sm font-medium text-[var(--color-text-primary)]">Create API key</h3>
        <label className="block text-[11px] text-[var(--color-text-muted)]">
          Label
          <input
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            className="mt-1 w-full px-2 py-1.5 text-[12px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
            placeholder="e.g. scout-sales"
          />
        </label>
        <div className="space-y-1">
          <p className="text-[11px] text-[var(--color-text-muted)]">Workspace grant</p>
          <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer">
            <SquareCheckbox checked={allWs} onChange={(e) => setAllWs(e.target.checked)} />
            All workspaces (*)
          </label>
          {!allWs && (
            <div className="max-h-32 overflow-y-auto border border-[var(--color-border)] p-2 space-y-1">
              {slugs.length === 0 ? (
                <p className="text-[10px] text-[var(--color-text-muted)]">No workspaces registered.</p>
              ) : (
                slugs.map(({ slug, label: lab }) => (
                  <label
                    key={slug}
                    className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer"
                  >
                    <SquareCheckbox
                      checked={selected.has(slug)}
                      onChange={(e) => {
                        setSelected((prev) => {
                          const next = new Set(prev)
                          if (e.target.checked) next.add(slug)
                          else next.delete(slug)
                          return next
                        })
                      }}
                    />
                    <span className="truncate">{lab}</span>
                    <span className="text-[10px] text-[var(--color-text-muted)] font-mono">{slug}</span>
                  </label>
                ))
              )}
            </div>
          )}
          {!allWs && selected.size === 0 && (
            <p className="text-[10px] text-[var(--color-status-warning-soft)]">
              No workspaces selected — key will authorize zero /v1 workspaces (fail-closed).
            </p>
          )}
        </div>
        <div className="space-y-1">
          <p className="text-[11px] text-[var(--color-text-muted)]">Capabilities</p>
          {(
            [
              ['hostSessions', hostSessions, setHostSessions, 'host-sessions'],
              ['canonicalMessage', canonicalMessage, setCanonicalMessage, 'canonical-message'],
              ['sandboxes', sandboxes, setSandboxes, 'sandboxes'],
              ['db', db, setDb, 'db (create, migrate and dump workspace databases)'],
            ] as const
          ).map(([key, val, set, lab]) => (
            <label
              key={key}
              className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer"
            >
              <SquareCheckbox checked={val} onChange={(e) => set(e.target.checked)} />
              {lab}
            </label>
          ))}
        </div>
        <div className="space-y-1" data-testid="hire-scope">
          <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer">
            <SquareCheckbox checked={hire} onChange={(e) => setHire(e.target.checked)} aria-label="Allow hiring agents" />
            Allow hiring agents (POST /v1/agents/hire)
          </label>
          {hire && (
            <div className="border border-[var(--color-border)] p-2 space-y-1.5">
              <p className="text-[10px] text-[var(--color-text-muted)]">
                Agents this key hires never get more than the key allows. Only the App access limit can be
                changed later; for the rest, mint a new key and revoke this one. {hireConfirmCopy()}
              </p>
              {(
                [
                  ['maxMembersPerDay', 'Agents per day'],
                  ['maxLiveMembers', 'Live agents'],
                  ['maxMembersPerTeam', 'Agents per team'],
                ] as const
              ).map(([k, lab]) => (
                <label key={k} className="flex items-center justify-between gap-2 text-[11px] text-[var(--color-text-secondary)]">
                  {lab}
                  <input
                    aria-label={lab}
                    value={hireForm[k]}
                    onChange={(e) => setHireField(k, e.target.value)}
                    inputMode="numeric"
                    className="w-20 px-2 py-1 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
                  />
                </label>
              ))}
              <label className="flex items-center justify-between gap-2 text-[11px] text-[var(--color-text-secondary)]">
                Database access for hired agents
                <select
                  aria-label="Database access for hired agents"
                  value={hireForm.db}
                  onChange={(e) => setHireField('db', e.target.value as HireForm['db'])}
                  className="px-2 py-1 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
                >
                  <option value="off">off</option>
                  <option value="read" disabled={!db}>read</option>
                  <option value="write" disabled={!db}>write</option>
                </select>
              </label>
              <label className="flex items-center justify-between gap-2 text-[11px] text-[var(--color-text-secondary)]">
                Messages and sessions to its agents
                <select
                  aria-label="Reach"
                  value={hireForm.reach}
                  onChange={(e) => setHireField('reach', e.target.value as HireForm['reach'])}
                  className="px-2 py-1 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
                >
                  <option value="none">none (hire, link, retire only)</option>
                  <option value="message">message</option>
                  <option value="host-sessions">message + host sessions</option>
                </select>
              </label>
              <label className="flex items-center justify-between gap-2 text-[11px] text-[var(--color-text-secondary)]">
                Folder under the hire root
                <input
                  aria-label="Folder under the hire root"
                  value={hireForm.folder}
                  onChange={(e) => setHireField('folder', e.target.value)}
                  placeholder="(the root)"
                  className="w-32 px-2 py-1 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
                />
              </label>
              <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer">
                <SquareCheckbox checked={hireForm.link} onChange={(e) => setHireField('link', e.target.checked)} aria-label="May link teams" />
                May link teams (only to agents this key hired)
              </label>
              <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer">
                <SquareCheckbox checked={hireForm.retire} onChange={(e) => setHireField('retire', e.target.checked)} aria-label="May retire" />
                May retire the agents it hired
              </label>
              <AppAccessLimit form={ceiling} onChange={setCeiling} opts={ceilingOpts} optsError={ceilingOptsError} />
            </div>
          )}
        </div>
        <ConfirmPrompt
          confirm={confirm}
          submitting={submitting}
          onSubmit={() => void submitPassword()}
          buttonLabel="Confirm and create"
        />
        {err && <p className="text-[11px] text-[var(--color-status-error-soft)]">{err}</p>}
        <div className="flex justify-end gap-2 pt-1">
          <button
            type="button"
            onClick={onClose}
            className="px-3 py-1.5 text-[11px] text-[var(--color-text-muted)] cursor-pointer"
          >
            Cancel
          </button>
          <button
            type="button"
            disabled={submitting}
            onClick={() => void submit()}
            className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] cursor-pointer disabled:opacity-50"
          >
            {submitting ? 'Creating…' : 'Create'}
          </button>
        </div>
      </div>
    </div>
  )
}

/**
 * Change a hire key's App access limit. The daemon decides: lowering
 * (fewer rooms, a lower role, no App access) saves straight away; widening
 * answers `vault_confirm_required`, and this asks for the same owner
 * confirm as minting, then saves with the token.
 */
function EditCeilingModal({
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
  const [submitting, setSubmitting] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  useDialogEscape(onClose, !submitting)

  const save = async () => {
    setSubmitting(true)
    setErr(null)
    try {
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
      setErr(daemonErrorText(e))
    } finally {
      setSubmitting(false)
    }
  }

  const submitPassword = async () => {
    setSubmitting(true)
    setErr(null)
    try {
      await confirm.submitPassword()
    } catch (e) {
      setErr(daemonErrorText(e))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/50 p-4" role="dialog" aria-modal="true" aria-label="App access">
      <div className="w-full max-w-md max-h-full overflow-y-auto border border-[var(--color-border)] bg-[var(--color-bg)] p-4 space-y-3 shadow-xl" data-testid="edit-app-access">
        <h3 className="text-sm font-medium text-[var(--color-text-primary)]">
          App access for {keyRow.label || keyRow.id}
        </h3>
        <p className="text-[11px] text-[var(--color-text-secondary)]">
          Now: {ceilingSummary(stored, ceilingOpts)}
        </p>
        <p className="text-[10px] text-[var(--color-text-muted)]">
          Lowering it saves right away. Widening it (another App, a new room or a higher role) asks you to
          confirm, like creating the key. Logins already made keep what they have.
        </p>
        {stored.length > 1 && (
          <p className="text-[10px] text-[var(--color-status-warning-soft)]">
            This key names {stored.length} Apps (set from a terminal). Saving here keeps only the one shown.
          </p>
        )}
        <AppAccessLimit form={form} onChange={setForm} opts={ceilingOpts} optsError={ceilingOptsError} />
        <ConfirmPrompt
          confirm={confirm}
          submitting={submitting}
          onSubmit={() => void submitPassword()}
          buttonLabel="Confirm and save"
        />
        {err && <p className="text-[11px] text-[var(--color-status-error-soft)]">{err}</p>}
        <div className="flex justify-end gap-2 pt-1">
          <button
            type="button"
            onClick={onClose}
            className="px-3 py-1.5 text-[11px] text-[var(--color-text-muted)] cursor-pointer"
          >
            Cancel
          </button>
          <button
            type="button"
            disabled={submitting}
            onClick={() => void save()}
            className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] cursor-pointer disabled:opacity-50"
          >
            {submitting ? 'Saving…' : 'Save'}
          </button>
        </div>
      </div>
    </div>
  )
}

/** Workspace settings → API tab: keys that grant this workspace only. */
export function WorkspaceApiKeysPanel({
  workspaceSlug,
}: {
  workspaceSlug: string
}): React.JSX.Element {
  const openGlobal = () => useSettingsStore.getState().setSection('api-tokens')
  const [keys, setKeys] = useState<ApiKeyRow[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    setError(null)
    try {
      const d = await daemonCliGet<ListResponse>(primaryScope(), 'api-keys/list')
      const all = Array.isArray(d.keys) ? d.keys : []
      setKeys(all.filter((k) => keyGrantsWorkspace(k, workspaceSlug)))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setKeys([])
    } finally {
      setLoading(false)
    }
  }, [workspaceSlug])

  useEffect(() => {
    setLoading(true)
    void refresh()
  }, [refresh])

  const act = useCallback(
    async (id: string, action: 'disable' | 'enable' | 'revoke') => {
      if (action === 'revoke') {
        if (
          !window.confirm(
            'Permanently revoke this API key? It cannot be re-enabled — mint a new secret under Settings → API Keys.',
          )
        ) {
          return
        }
      }
      setBusyId(id)
      try {
        await daemonCliPost(primaryScope(), `api-keys/${action}`, { id })
        await refresh()
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusyId(null)
      }
    },
    [refresh],
  )

  return (
    <div className="space-y-4 max-w-3xl">
      <div>
        <h3 className="text-sm font-medium text-[var(--color-text-primary)]">API access</h3>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          Tokens that can call <code className="text-[10px]">/v1/w/{workspaceSlug}/…</code> on this
          daemon (grant includes this workspace or <code className="text-[10px]">*</code>).
        </p>
      </div>
      <button
        type="button"
        onClick={openGlobal}
        className="text-[11px] text-[var(--color-accent)] cursor-pointer"
      >
        Manage all tokens →
      </button>
      {error && <p className="text-[11px] text-[var(--color-status-error-soft)]">{error}</p>}
      {loading ? (
        <p className="text-[11px] text-[var(--color-text-muted)]">Loading…</p>
      ) : keys.length === 0 ? (
        <p className="text-[11px] text-[var(--color-text-muted)]">
          No API keys grant this workspace. Mint one under Settings → API Keys (or{' '}
          <code className="text-[10px]">k2 api-key create --workspaces &apos;{workspaceSlug}&apos;</code>
          ).
        </p>
      ) : (
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          {keys.map((k) => {
            const state = keyState(k)
            const busy = busyId === k.id
            return (
              <div key={k.id} className="p-3 flex flex-col sm:flex-row sm:justify-between gap-2">
                <div className="min-w-0 space-y-0.5">
                  <div className="flex items-center gap-2">
                    <span className="text-[12px] font-medium text-[var(--color-text-primary)]">
                      {k.label || '(no label)'}
                    </span>
                    {stateBadge(state)}
                  </div>
                  <p className="text-[10px] font-mono text-[var(--color-text-muted)] break-all">
                    {k.id}
                  </p>
                  <p className="text-[11px] text-[var(--color-text-secondary)]">
                    {formatGrant(k.allowedWorkspaces)} · {capsSummary(k.capabilities)}
                  </p>
                </div>
                <div className="flex gap-1.5 flex-shrink-0">
                  {state === 'active' && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void act(k.id, 'disable')}
                      className="px-2 py-1 text-[10px] border border-[var(--color-border)] cursor-pointer disabled:opacity-50"
                    >
                      Disable
                    </button>
                  )}
                  {state === 'disabled' && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void act(k.id, 'enable')}
                      className="px-2 py-1 text-[10px] border border-[var(--color-border)] cursor-pointer disabled:opacity-50"
                    >
                      Enable
                    </button>
                  )}
                  {state !== 'revoked' && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void act(k.id, 'revoke')}
                      className="px-2 py-1 text-[10px] border border-[var(--color-border)] text-[var(--color-status-error-soft)] cursor-pointer disabled:opacity-50"
                    >
                      Revoke
                    </button>
                  )}
                </div>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
