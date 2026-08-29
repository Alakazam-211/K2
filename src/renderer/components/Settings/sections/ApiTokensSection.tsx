// Settings → K2 API Tokens — daemon-wide inventory of k2sk_ keys.
// Owner-tier routes (/cli/api-keys/*). Secret shown once on create only.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import type { SettingEntry } from '../searchManifest'
import {
  capsSummary,
  formatGrant,
  keyGrantsWorkspace,
  keyState,
  type ApiKeyRow,
} from './api-keys-api'

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
  const [keys, setKeys] = useState<ApiKeyRow[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [createOpen, setCreateOpen] = useState(false)
  const [mintedSecret, setMintedSecret] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    setError(null)
    try {
      const d = await daemonCliGet<ListResponse>('api-keys/list')
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
        await daemonCliPost(`api-keys/${action}`, { id })
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
  const [submitting, setSubmitting] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const slugs = useMemo(
    () =>
      workspaceOptions.map((p) => ({
        slug: (p.name || p.path.split(/[/\\]/).pop() || p.id).trim(),
        label: p.name || p.path,
      })),
    [workspaceOptions],
  )

  const submit = async () => {
    setSubmitting(true)
    setErr(null)
    try {
      const body: Record<string, unknown> = {
        label: label.trim() || undefined,
        capabilities: {
          hostSessions,
          canonicalMessage,
          sandboxes,
        },
      }
      if (allWs) {
        body.workspaces = '*'
      } else if (selected.size > 0) {
        body.workspaces = [...selected]
      }
      const res = await daemonCliPost<{ id?: string; key?: string; error?: string }>(
        'api-keys/create',
        body,
      )
      if (!res.key) throw new Error(res.error || 'create returned no key')
      onCreated(res.key)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
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
            <input type="checkbox" checked={allWs} onChange={(e) => setAllWs(e.target.checked)} />
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
                    <input
                      type="checkbox"
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
            ] as const
          ).map(([key, val, set, lab]) => (
            <label
              key={key}
              className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer"
            >
              <input type="checkbox" checked={val} onChange={(e) => set(e.target.checked)} />
              {lab}
            </label>
          ))}
        </div>
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
      const d = await daemonCliGet<ListResponse>('api-keys/list')
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
        await daemonCliPost(`api-keys/${action}`, { id })
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
