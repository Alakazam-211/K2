// Settings → Compute, node admin (prd-k2-node-admin-v1 S10).
//
// Agents propose, the owner approves, the node checks. These parts only read
// daemon truth and send the owner's gestures:
//   - ApprovalsPanel: requests waiting for the owner, in plain words with any
//     root script shown in full. Approve = a fresh owner confirm (Touch ID /
//     login password on the Mac, the Connect or vault password elsewhere)
//     for THAT request, then the daemon signs and sends. Deny with a reason.
//   - NodeAdminPanel: one node's health, pending approvals, k2-node update
//     state with Update / Roll back, and its offline alert.
// Polling happens in effects while a part is open, never in render paths.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Callout, Input } from '@/components/ui'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import {
  approveAdminRequest,
  denyAdminRequest,
  fetchAdminRequests,
  fetchNodeHealth,
  finishAdminConfirm,
  healthLines,
  proposerLabel,
  requestScripts,
  requestSteps,
  rollbackNode,
  setNodeOfflineAlert,
  startAdminConfirm,
  updateNode,
  type AdminRequest,
  type NodeHealthView,
} from './compute-admin-api'

const POLL_MS = 5000
const muted = 'text-[var(--color-text-muted)]'
const secondary = 'text-[var(--color-text-secondary)]'

function nowSec(): number {
  return Math.floor(Date.now() / 1000)
}

/** An older daemon (no admin routes) or compute off: render nothing. */
function isMissingRoute(e: unknown): boolean {
  const code = daemonErrorOf(e)?.code
  return code === 'not_found' || code === 'compute_disabled'
}

// ── approvals ────────────────────────────────────────────────────────

export function ApprovalsPanel({ onChanged }: { onChanged?: () => void }): React.JSX.Element | null {
  const [rows, setRows] = useState<AdminRequest[] | null>(null)
  const [canApprove, setCanApprove] = useState(false)
  const [hidden, setHidden] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const r = await fetchAdminRequests('open')
      setRows(r.requests)
      setCanApprove(r.canApprove)
      setError(null)
    } catch (e) {
      if (isMissingRoute(e)) {
        setHidden(true)
        return
      }
      setError(daemonErrorText(e))
    }
  }, [])

  useEffect(() => {
    void load()
    const t = setInterval(() => void load(), POLL_MS)
    return () => clearInterval(t)
  }, [load])

  const changed = useCallback(async () => {
    await load()
    onChanged?.()
  }, [load, onChanged])

  if (hidden) return null
  const pending = (rows ?? []).filter((r) => r.state === 'pending')
  const inFlight = (rows ?? []).filter((r) => r.state === 'approved' || r.state === 'sent')
  return (
    <div className="space-y-2" data-testid="compute-approvals" data-settings-id="compute.approvals">
      <div className="text-xs font-medium text-[var(--color-text-primary)]">
        Approvals{pending.length > 0 ? ` (${pending.length})` : ''}
      </div>
      <p className={`text-[11px] ${muted}`}>
        Agents can only ask. Root work on a node waits here until you approve it, with Touch ID or your password, each time.
      </p>
      {error ? <Callout tone="error">{error}</Callout> : null}
      {rows && pending.length === 0 && inFlight.length === 0 ? (
        <p className={`text-[11px] ${muted}`}>Nothing is waiting.</p>
      ) : null}
      {pending.map((r) => (
        <RequestCard key={r.id} request={r} canApprove={canApprove} onChanged={changed} />
      ))}
      {inFlight.map((r) => (
        <div key={r.id} className={`text-[11px] ${secondary}`} data-testid={`compute-request-inflight-${r.id}`}>
          {r.state === 'approved' ? 'Approved, waiting for the node' : 'Running on the node'}: {r.summary}
        </div>
      ))}
    </div>
  )
}

type ConfirmStep = { step: 'idle' } | { step: 'starting' } | { step: 'mac' } | { step: 'password'; method: string }

function RequestCard({
  request: r,
  canApprove,
  onChanged,
}: {
  request: AdminRequest
  canApprove: boolean
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const [confirm, setConfirm] = useState<ConfirmStep>({ step: 'idle' })
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [denying, setDenying] = useState(false)
  const [why, setWhy] = useState('')
  const [showArgs, setShowArgs] = useState(false)
  const pwRef = useRef<HTMLInputElement>(null)
  const [hasPw, setHasPw] = useState(false)
  const scripts = requestScripts(r)
  const steps = requestSteps(r)

  const finish = async (method: string, password?: string): Promise<void> => {
    const { token } = await finishAdminConfirm(r.id, method, r.summary, password)
    await approveAdminRequest(r.id, token)
    setConfirm({ step: 'idle' })
    await onChanged()
  }

  const approve = async (): Promise<void> => {
    setError(null)
    setConfirm({ step: 'starting' })
    try {
      const { methods } = await startAdminConfirm(r.id)
      if (methods.includes('local-auth')) {
        setConfirm({ step: 'mac' })
        await finish('local-auth')
        return
      }
      const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
      if (pw) {
        setConfirm({ step: 'password', method: pw })
        return
      }
      setConfirm({ step: 'idle' })
      setError('No way to confirm here yet. Set a vault password from a terminal outside K2: k2 vault confirm-password set.')
    } catch (e) {
      setConfirm({ step: 'idle' })
      setError(daemonErrorText(e))
    }
  }

  const submitPassword = async (method: string): Promise<void> => {
    const el = pwRef.current
    if (!el) return
    const pw = el.value
    el.value = ''
    setHasPw(false)
    setError(null)
    try {
      await finish(method, pw)
    } catch (e) {
      setConfirm({ step: 'idle' })
      setError(daemonErrorText(e))
    }
  }

  const deny = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await denyAdminRequest(r.id, why.trim() || undefined)
      setDenying(false)
      await onChanged()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="border border-[var(--color-border)] p-3 space-y-2" data-testid={`compute-request-${r.id}`}>
      <div className="text-xs text-[var(--color-text-primary)]">{r.summary}</div>
      <div className={`text-[11px] ${secondary} flex flex-wrap gap-x-3`}>
        <span>{r.node ?? r.nodeId}</span>
        <span>asked by {proposerLabel(r)}</span>
        <span>{formatRelativeTime(r.proposedAt, nowSec())}</span>
        <span className={muted}>{r.class === 'privileged' ? 'root' : 'operate'}</span>
      </div>
      {r.reason ? <div className={`text-[11px] ${secondary}`}>Why: {r.reason}</div> : null}
      {scripts.map((s, i) => (
        <div key={i} className="space-y-1">
          <div className={`text-[11px] ${muted}`}>Runs as root with {s.interpreter}. Read it all:</div>
          <pre
            className="text-[11px] font-mono whitespace-pre-wrap break-all max-h-64 overflow-auto border border-[var(--color-border)] p-2 text-[var(--color-text-primary)]"
            data-testid="compute-request-script"
          >
            {s.text}
          </pre>
        </div>
      ))}
      {steps.length > 0 ? (
        <button type="button" className={`text-[10px] ${muted} underline`} onClick={() => setShowArgs((v) => !v)}>
          {showArgs ? 'Hide exact request' : 'Show exact request'}
        </button>
      ) : null}
      {showArgs ? (
        <pre className="text-[10px] font-mono whitespace-pre-wrap break-all text-[var(--color-text-secondary)]" data-testid="compute-request-args">
          {JSON.stringify(steps, null, 2)}
          {`\n${r.digest}`}
        </pre>
      ) : null}

      {confirm.step === 'starting' ? <p className={`text-[11px] ${muted}`}>Asking for your confirm…</p> : null}
      {confirm.step === 'mac' ? <p className={`text-[11px] ${muted}`}>Confirm on your Mac.</p> : null}
      {confirm.step === 'password' ? (
        <form
          className="flex flex-col gap-1.5 max-w-sm"
          onSubmit={(e) => {
            e.preventDefault()
            void submitPassword(confirm.method)
          }}
        >
          <label className={`text-[11px] ${muted}`}>
            {confirm.method === 'connect-password' ? 'Your K2 Connect password' : 'Your vault password'}
            <input
              ref={pwRef}
              type="password"
              autoComplete="current-password"
              data-testid="compute-approve-password"
              onChange={(e) => setHasPw(e.currentTarget.value !== '')}
              className="mt-1 w-full border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs"
            />
          </label>
          <div className="flex gap-1.5">
            <Button type="submit" variant="accent" disabled={!hasPw}>
              Approve
            </Button>
            <Button type="button" variant="ghost" onClick={() => setConfirm({ step: 'idle' })}>
              Cancel
            </Button>
          </div>
        </form>
      ) : null}

      {confirm.step === 'idle' ? (
        denying ? (
          <div className="flex flex-wrap items-center gap-2">
            <Input value={why} onChange={(e) => setWhy(e.target.value)} placeholder="Why (the agent reads this)" aria-label="Why deny" className="w-64" />
            <Button disabled={busy} onClick={() => void deny()}>
              Deny
            </Button>
            <Button variant="ghost" onClick={() => setDenying(false)}>
              Cancel
            </Button>
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-2">
            {canApprove ? (
              <Button variant="accent" disabled={busy} onClick={() => void approve()} data-testid={`compute-approve-${r.id}`}>
                Approve
              </Button>
            ) : (
              <span className={`text-[11px] ${muted}`}>Only the server's owner approves.</span>
            )}
            <Button disabled={busy} onClick={() => setDenying(true)} data-testid={`compute-deny-${r.id}`}>
              Deny
            </Button>
            <span className={`text-[10px] ${muted}`}>expires {formatRelativeTime(r.expiresAt, nowSec())}</span>
          </div>
        )
      ) : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

// ── one node's admin view ────────────────────────────────────────────

export function NodeAdminPanel({ node }: { node: string }): React.JSX.Element | null {
  const [open, setOpen] = useState(false)
  const [view, setView] = useState<NodeHealthView | null>(null)
  const [hidden, setHidden] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const load = useCallback(async () => {
    try {
      setView(await fetchNodeHealth(node))
      setError(null)
    } catch (e) {
      if (isMissingRoute(e)) {
        setHidden(true)
        return
      }
      setError(daemonErrorText(e))
    }
  }, [node])

  useEffect(() => {
    if (!open) return
    void load()
    const t = setInterval(() => void load(), POLL_MS)
    return () => clearInterval(t)
  }, [open, load])

  const act = async (fn: () => Promise<{ note?: string }>): Promise<void> => {
    setBusy(true)
    setError(null)
    setNote(null)
    try {
      const out = await fn()
      if (out.note) setNote(out.note)
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
      void load()
    }
  }

  if (hidden) return null
  if (!open) {
    return (
      <button type="button" className={`text-[11px] ${secondary} underline`} onClick={() => setOpen(true)} data-testid={`compute-node-admin-open-${node}`}>
        Health and admin
      </button>
    )
  }
  const lines = healthLines(view?.facts ?? null)
  const canUpdate = (view?.features ?? []).includes('self-update')
  const u = view?.update
  return (
    <div className="space-y-1.5 border-t border-[var(--color-border)] pt-2" data-testid={`compute-node-admin-${node}`}>
      <div className="flex items-center gap-2">
        <span className="text-[11px] font-medium text-[var(--color-text-primary)]">Health and admin</span>
        <button type="button" className={`text-[10px] ${muted} underline`} onClick={() => setOpen(false)}>
          Hide
        </button>
      </div>
      {!view && !error ? <p className={`text-[11px] ${muted}`}>Loading…</p> : null}
      {view ? (
        <>
          <div className={`text-[11px] ${secondary}`}>
            k2-node {view.nodeVersion ?? '?'} · remote admin {view.remoteAdmin ?? 'unknown'}
            {view.at ? ` · reported ${formatRelativeTime(view.at, nowSec())}` : ''}
          </div>
          {lines.length > 0 ? (
            <ul className={`text-[11px] ${secondary} list-none space-y-0.5`} data-testid="compute-node-health">
              {lines.map((l) => (
                <li key={l}>{l}</li>
              ))}
            </ul>
          ) : (
            <p className={`text-[11px] ${muted}`}>No health report yet (this k2-node may predate node admin).</p>
          )}
          {view.pendingApprovals > 0 ? (
            <div className="text-[11px] text-[var(--color-status-warn-amber-soft)]" data-testid="compute-node-pending">
              {view.pendingApprovals} request(s) wait for your approval above.
            </div>
          ) : null}
          <div className={`text-[11px] ${secondary}`} data-testid="compute-node-update">
            Update: {u ? `${u.state}${u.version ? ` ${u.version}` : ''}${u.reason ? ` (${u.reason})` : ''}` : 'nothing yet'}
            {view.updateRing === 'canary' ? ' · canary' : ''}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {canUpdate ? (
              <>
                <Button disabled={busy || !view.online} onClick={() => void act(() => updateNode(node))}>
                  Update k2-node
                </Button>
                <Button variant="quiet" disabled={busy || !view.online} onClick={() => void act(() => rollbackNode(node))}>
                  Roll back
                </Button>
              </>
            ) : (
              <span className={`text-[11px] ${muted}`}>Remote update needs a k2-node with self-update.</span>
            )}
            <Button
              disabled={busy}
              onClick={() => void act(async () => setNodeOfflineAlert(node, view.alertAfterSecs ? null : 600).then(() => ({})))}
              data-testid={`compute-node-alert-${node}`}
            >
              {view.alertAfterSecs ? `Offline alert after ${Math.round(view.alertAfterSecs / 60)}m: turn off` : 'Alert me when it goes offline'}
            </Button>
          </div>
          <div className={`text-[10px] ${muted}`}>
            Alerts go to {view.alertsTo ?? 'nobody yet (k2 compute node alerts --to <workspace>)'}. They only fire while this server is awake.
          </div>
        </>
      ) : null}
      {note ? <p className={`text-[11px] ${muted}`}>{note}</p> : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}
