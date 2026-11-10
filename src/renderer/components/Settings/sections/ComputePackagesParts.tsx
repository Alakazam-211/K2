// Settings → Compute, package requests (prd-compute-self-serve-environments-v1
// §6, §12.3). Cards in chat are the main path; this is the list.
//   - PackageRequestsPanel: under Approvals, one card per batch of pending
//     package requests (what installs, where, for whom, why, the dry run's
//     size), with Allow once / Allow and add to the approved list / Deny.
//     Recent decided requests below, collapsed.
//   - NodePackagesSummary: one node's host line, package manager, policy,
//     open requests and the reboot-pending badge (from the nodes list row).
//   - LocalPackagesSummary: this machine's policy and open requests, when
//     this machine is one of the listed nodes.
// Reads happen in effects (a slow poll plus the daemon's `sync:settings
// {compute}` signal), never in render paths. The daemon decides everything.

import React, { useCallback, useEffect, useState } from 'react'
import { Button, Callout, Input } from '@/components/ui'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import type { ComputeNode } from './compute-api'
import {
  decidePkgRequest,
  fetchNodePackages,
  fetchPkgRequests,
  groupPkgBatches,
  onComputeSync,
  pkgAskerLabel,
  pkgDepsLine,
  type NodePackagesView,
  type PkgBatch,
  type PkgRequestRow,
} from './compute-packages-api'

const POLL_MS = 5000
const muted = 'text-[var(--color-text-muted)]'
const secondary = 'text-[var(--color-text-secondary)]'
const RECENT_LIMIT = 10

export const HOST_WIDE_NOTE = 'This is installed for every job on that machine.'

function nowSec(): number {
  return Math.floor(Date.now() / 1000)
}

/** An older daemon (no package routes) or compute off: render nothing. */
function isMissingRoute(e: unknown): boolean {
  const code = daemonErrorOf(e)?.code
  return code === 'not_found' || code === 'compute_disabled'
}

// ── Package requests (under Approvals) ─────────────────────────────────

export function PackageRequestsPanel({ onChanged }: { onChanged?: () => void }): React.JSX.Element | null {
  const [pending, setPending] = useState<PkgRequestRow[] | null>(null)
  const [recent, setRecent] = useState<PkgRequestRow[]>([])
  const [canDecide, setCanDecide] = useState(false)
  const [hidden, setHidden] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [p, all] = await Promise.all([fetchPkgRequests('pending'), fetchPkgRequests('all', { limit: 30 })])
      setPending(p.requests.filter((r) => r.state === 'pending'))
      setRecent(all.requests.filter((r) => r.state !== 'pending').slice(0, RECENT_LIMIT))
      setCanDecide(p.canDecide)
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
    const t = setInterval(() => {
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return
      void load()
    }, POLL_MS)
    const off = onComputeSync((kind) => {
      if (kind === 'pkg_request') void load()
    })
    return () => {
      clearInterval(t)
      off()
    }
  }, [load])

  const changed = useCallback(async () => {
    await load()
    onChanged?.()
  }, [load, onChanged])

  if (hidden) return null
  const batches = groupPkgBatches(pending ?? [])
  return (
    <div className="space-y-2" data-testid="compute-pkg-requests" data-settings-id="compute.packages">
      <div className="type-name text-[var(--color-text-primary)]">
        Package requests{batches.length > 0 ? ` (${batches.length})` : ''}
      </div>
      <p className={`type-hint ${muted}`}>
        Agents ask for system packages a job needs. The node checks each one with a dry run before it installs.
      </p>
      {error ? <Callout tone="error">{error}</Callout> : null}
      {pending && batches.length === 0 ? <p className={`type-hint ${muted}`}>Nothing is waiting.</p> : null}
      {batches.length > 0 && !canDecide ? (
        <p className={`type-hint ${muted}`}>Only this server&apos;s owner or an Admin decides these.</p>
      ) : null}
      {batches.map((b) => (
        <PkgBatchCard key={b.batchId} batch={b} canDecide={canDecide} onChanged={changed} />
      ))}
      {recent.length > 0 ? (
        <details data-testid="compute-pkg-recent">
          <summary className={`type-hint ${secondary} cursor-pointer`}>Recent decisions ({recent.length})</summary>
          <div className="space-y-1 pt-1">
            {recent.map((r) => (
              <div
                key={r.id}
                className="type-hint flex flex-wrap gap-x-3 border-b border-[var(--color-border)] py-1"
                data-testid={`compute-pkg-recent-${r.id}`}
              >
                <span className="text-[var(--color-text-primary)]">{r.state}</span>
                <span className="font-mono text-[var(--color-text-secondary)]">{r.names.join(', ')}</span>
                <span className={muted}>
                  on {r.node} for {pkgAskerLabel(r)}
                </span>
                {r.decidedBy ? <span className={muted}>by {r.decidedBy}</span> : null}
                {r.note ? <span className={secondary}>{r.note}</span> : null}
                <span className={muted}>{formatRelativeTime(r.decidedAt ?? r.createdAt, nowSec())}</span>
              </div>
            ))}
          </div>
        </details>
      ) : null}
    </div>
  )
}

export function PkgBatchCard({
  batch: b,
  canDecide,
  onChanged,
}: {
  batch: PkgBatch
  canDecide: boolean
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<string | null>(null)
  const [denying, setDenying] = useState(false)
  const [note, setNote] = useState('')
  const deps = pkgDepsLine(b)

  const decide = async (approve: boolean, add = false): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const out = await decidePkgRequest(b.batchId, approve, { add, note: approve ? undefined : note.trim() || undefined })
      const notes = (out?.rows ?? []).map((r) => r.note).filter((x): x is string => !!x)
      setResult(notes.length > 0 ? notes.join(' · ') : null)
      setDenying(false)
      await onChanged()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="border border-[var(--color-border)] p-3 space-y-2" data-testid={`compute-pkg-batch-${b.batchId}`}>
      <div className="type-row text-[var(--color-text-primary)]">
        Install <span className="font-mono">{b.names.join(', ')}</span>
      </div>
      <div className={`type-hint ${secondary} space-y-0.5`}>
        {b.nodes.map((n) => (
          <div key={n.node} data-testid="compute-pkg-batch-node">
            on <span className="font-medium">{n.node}</span>
            {n.host ? <span className={muted}> ({n.host})</span> : null}
          </div>
        ))}
      </div>
      <div className={`type-hint ${secondary} flex flex-wrap gap-x-3`}>
        <span>for {b.who}</span>
        {deps ? <span data-testid="compute-pkg-batch-deps">{deps}</span> : null}
        <span className={muted}>{formatRelativeTime(b.createdAt, nowSec())}</span>
      </div>
      {b.why ? <p className={`type-hint ${secondary}`}>Why: {b.why}</p> : null}
      {b.otherServer ? (
        <p className={`type-hint ${muted}`} data-testid="compute-pkg-batch-hostwide">
          {HOST_WIDE_NOTE}
        </p>
      ) : null}
      {b.refusals.map((r) => (
        <Callout key={r} tone="warn">
          The dry run would refuse this: {r}
        </Callout>
      ))}
      {canDecide ? (
        denying ? (
          <div className="flex flex-wrap items-center gap-2">
            <Input
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder="Note for the agent (optional)"
              aria-label="Note for the agent"
              className="w-64"
            />
            <Button disabled={busy} onClick={() => void decide(false)} data-testid={`compute-pkg-deny-confirm-${b.batchId}`}>
              Deny
            </Button>
            <Button variant="ghost" onClick={() => setDenying(false)}>
              Cancel
            </Button>
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-2">
            <Button variant="accent" disabled={busy} onClick={() => void decide(true)} data-testid={`compute-pkg-allow-${b.batchId}`}>
              Allow once
            </Button>
            <Button disabled={busy} onClick={() => void decide(true, true)} data-testid={`compute-pkg-allow-add-${b.batchId}`}>
              Allow and add to the approved list
            </Button>
            <Button variant="quiet" disabled={busy} onClick={() => setDenying(true)} data-testid={`compute-pkg-deny-${b.batchId}`}>
              Deny
            </Button>
          </div>
        )
      ) : null}
      {result ? <p className={`type-hint ${secondary}`}>{result}</p> : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

// ── Per node ─────────────────────────────────────────────────────────

export function NodePackagesSummary({ node }: { node: ComputeNode }): React.JSX.Element | null {
  const hostLine = node.host?.line ?? null
  const pkg = node.packages ?? null
  const reboot = node.rebootPending === true || node.host?.rebootRequired === true
  if (!hostLine && !pkg && !reboot) return null
  return (
    <div className={`type-hint ${secondary} space-y-0.5`} data-testid={`compute-node-packages-${node.name}`}>
      {hostLine || reboot ? (
        <div className="flex flex-wrap items-center gap-x-2">
          {hostLine ? <span data-testid="compute-node-host">{hostLine}</span> : null}
          {reboot ? (
            <span
              className="type-hint text-[var(--color-status-warn-amber-soft)] border border-[var(--color-border)] px-1"
              data-testid="compute-node-reboot"
            >
              reboot pending
            </span>
          ) : null}
        </div>
      ) : null}
      {pkg ? (
        <div className="flex flex-wrap gap-x-3">
          <span>Packages: {pkg.manager ?? 'no package manager'}</span>
          {pkg.policy ? <span>policy {pkg.policy}</span> : null}
          <span data-testid="compute-node-open-requests">
            {pkg.openRequests ?? 0} open {(pkg.openRequests ?? 0) === 1 ? 'request' : 'requests'}
          </span>
        </div>
      ) : null}
    </div>
  )
}

// ── This machine ─────────────────────────────────────────────────────

/** This machine's package policy and open requests, read from the server
 *  that lists it as a node. */
export function LocalPackagesSummary({ node }: { node: ComputeNode }): React.JSX.Element | null {
  const [view, setView] = useState<NodePackagesView | null>(null)
  const [hidden, setHidden] = useState(false)

  const load = useCallback(async () => {
    try {
      setView(await fetchNodePackages(node.name))
    } catch (e) {
      if (isMissingRoute(e)) setHidden(true)
    }
  }, [node.name])

  useEffect(() => {
    void load()
    const off = onComputeSync((kind) => {
      if (kind === 'pkg_request') void load()
    })
    return off
  }, [load])

  if (hidden) return null
  const pkg = view?.packages ?? node.packages ?? null
  const policy = pkg?.policy ?? null
  const open = (view?.requests ?? []).filter((r) => r.state === 'pending')
  const openCount = view?.packages?.openRequests ?? node.packages?.openRequests ?? open.length
  return (
    <div className="space-y-1" data-testid="compute-local-packages">
      {policy ? <p className={`type-hint ${secondary}`}>Package policy for {node.name}: {policy}</p> : null}
      <p className={`type-hint ${secondary}`}>
        {openCount} open package {openCount === 1 ? 'request' : 'requests'}
        {openCount > 0 ? ': allow or deny them under Package requests above.' : '.'}
      </p>
      {open.length > 0 ? (
        <ul className={`type-hint ${secondary}`}>
          {open.map((r) => (
            <li key={r.id} data-testid={`compute-local-pkg-${r.id}`}>
              <span className="font-mono">{r.names.join(', ')}</span> for {pkgAskerLabel(r)}
              {r.why ? <span className={muted}> ({r.why})</span> : null}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  )
}
