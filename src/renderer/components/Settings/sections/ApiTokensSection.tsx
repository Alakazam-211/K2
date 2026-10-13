// Settings → K2 API Tokens — daemon-wide inventory of k2sk_ keys.
// Owner-tier routes (/cli/api-keys/*). Secret shown once on create only.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import type { SettingEntry } from '../searchManifest'
import {
  capsSummary,
  finishHireKeyConfirm,
  formatGrant,
  HIRE_FORM_DEFAULTS,
  hireLimitsFromForm,
  hireSummary,
  keyGrantsWorkspace,
  keyState,
  startHireKeyConfirm,
  type ApiKeyRow,
  type HireForm,
} from './api-keys-api'
import { primaryScope } from '@/kessel/server-scope'
import { SquareCheckbox } from '@/components/ui'

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
        {!apiEnabled && (
          <p className="text-[11px] text-[var(--color-status-warning-soft)] mt-1 max-w-2xl" data-testid="api-surface-off">
            The public /v1 API is off on this server, so no key works yet (hiring agents included).
            Turn it on under Settings → K2 Connect → Public API, or <code className="text-[10px]">k2 api on</code>.
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
                </div>
                <div className="flex flex-wrap gap-1.5 flex-shrink-0">
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
          onClose={() => setCreateOpen(false)}
          onCreated={(secret) => {
            setCreateOpen(false)
            setMintedSecret(secret)
            void refresh()
          }}
        />
      )}
    </div>
  )
}

function CreateKeyModal({
  workspaceOptions,
  onClose,
  onCreated,
}: {
  workspaceOptions: Array<{ id: string; name: string; path: string }>
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
  const [confirmStep, setConfirmStep] = useState<'idle' | 'mac' | 'password'>('idle')
  const [pwMethod, setPwMethod] = useState('vault-password')
  const pwRef = useRef<HTMLInputElement>(null)
  const [submitting, setSubmitting] = useState(false)
  const [err, setErr] = useState<string | null>(null)
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
      body.hire = hireLimitsFromForm(hireForm, db)
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
      hireLimitsFromForm(hireForm, db)
      // A key that can hire agents needs a fresh owner confirm (Touch ID /
      // login password, or the vault password), owner token included.
      const { methods } = await startHireKeyConfirm()
      if (methods.includes('local-auth')) {
        setConfirmStep('mac')
        const { token } = await finishHireKeyConfirm('local-auth')
        setConfirmStep('idle')
        await mint(token)
        return
      }
      const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
      if (pw) {
        setPwMethod(pw)
        setConfirmStep('password')
        return
      }
      setErr('No way to confirm here yet. Set a vault password from a terminal outside K2: k2 vault confirm-password set.')
    } catch (e) {
      setConfirmStep('idle')
      setErr(daemonErrorText(e))
    } finally {
      setSubmitting(false)
    }
  }

  const submitPassword = async () => {
    const el = pwRef.current
    if (!el) return
    const pw = el.value
    el.value = ''
    setSubmitting(true)
    setErr(null)
    try {
      const { token } = await finishHireKeyConfirm(pwMethod, pw)
      setConfirmStep('idle')
      await mint(token)
    } catch (e) {
      setErr(daemonErrorText(e))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/50 p-4">
      <div className="w-full max-w-md border border-[var(--color-border)] bg-[var(--color-bg)] p-4 space-y-3 shadow-xl">
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
                Agents this key hires never get more than the key allows. Limits can&apos;t be edited later:
                mint a new key and revoke this one. Creating it asks you to confirm (Touch ID or a password).
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
              <p className="text-[10px] text-[var(--color-status-warning-soft)]" data-testid="hire-whole-room-warning">
                App logins for hired agents cover whole rooms until folder grants exist. Name the Apps a key
                may give logins on from a terminal: k2 api-key create --hire --app-login APP[:ROOMS[:ROLES]].
              </p>
            </div>
          )}
        </div>
        {confirmStep === 'mac' && (
          <p className="text-[11px] text-[var(--color-text-secondary)]">Confirm on this Mac (Touch ID or your login password)…</p>
        )}
        {confirmStep === 'password' && (
          <div className="space-y-1">
            <p className="text-[11px] text-[var(--color-text-secondary)]">
              {pwMethod === 'connect-password' ? 'Your login password' : 'Vault password'} to confirm
            </p>
            <input
              ref={pwRef}
              type="password"
              aria-label="Confirm password"
              className="w-full px-2 py-1.5 text-[12px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
              onKeyDown={(e) => {
                if (e.key === 'Enter') void submitPassword()
              }}
            />
            <button
              type="button"
              disabled={submitting}
              onClick={() => void submitPassword()}
              className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] cursor-pointer disabled:opacity-50"
            >
              Confirm and create
            </button>
          </div>
        )}
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
