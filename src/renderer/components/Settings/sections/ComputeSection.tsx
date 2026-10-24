// Settings → Compute (prd-k2-compute-nodes-v1, preview).
//
// Daemon-first: every fact on this page is read from `/cli/compute/*` and
// every button is one owner gesture posted back. Nothing here schedules,
// sweeps or decides; the polling below only re-reads truth while the page is
// open (effects, never render paths).
//
//   A. Preview switch — `computePreview` on the focused server. While off the
//      compute routes 404 `compute_disabled` and the controller part is just
//      the switch and one paragraph.
//   B. Nodes of this server — the focused server (`primaryScope()`) as the
//      controller: nodes, confirm/pause/rename/remove, grants, enroll codes,
//      recent jobs and audit lines.
//      Which agents may use compute is NOT set on this page: it's the Agent
//      tab's "Allow compute nodes" (Rosson 2026-10-11); node details only
//      show that switch read-only.
//   C. This computer — desktop only, this machine's own daemon
//      (`scopeForHost('local')`): make it a node, the owner's word over it.
//   D. One-click pairing on the same Connect account (inside C's sheet).
//   E. Node admin (prd-k2-node-admin-v1 S10, `ComputeAdminParts`): the
//      owner's Approvals above the nodes, and per node its health, pending
//      approvals, k2-node update state and offline alert.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { SettingsGroup, SettingDropdown } from '../controls/SettingControls'
import { Button, Callout, Input, SquareCheckbox, Toggle } from '@/components/ui'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import { useConnectHostStore } from '@/stores/connect-host'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { webFeatures } from '@/web/features'
import { isAirgap } from '@/lib/airgap'
import { formatRelativeTime } from '@/lib/format-relative-time'
import {
  computeErrorMessage,
  confirmComputeNode,
  controllerHost,
  disableLocalNode,
  drainLocalNode,
  enableLocalNode,
  fetchComputeEvents,
  fetchComputeGrants,
  fetchComputeJobs,
  fetchComputeNodes,
  fetchLocalStatus,
  formatBytes,
  formatCores,
  isComputeDisabled,
  mintEnrollCode,
  nodeDot,
  pauseComputeNode,
  pauseLocalNode,
  removeComputeNode,
  renameComputeNode,
  resumeLocalNode,
  remoteAdminLevel,
  revokeComputeGrant,
  setComputeGrant,
  setLocalPolicy,
  setLocalRemoteAdmin,
  validNodeName,
  type ComputeEvent,
  type ComputeGrant,
  type ComputeJob,
  type ComputeNode,
  type ComputeNodesList,
  type EnrollCode,
  type GrantLimits,
  type LocalDisableResult,
  type LocalEnableResult,
  type LocalRemoteAdminResult,
  type LocalStatus,
  type RemoteAdminLevel,
  type NodeDot,
  type PolicyKey,
} from './compute-api'
import { getAccountAccessToken } from './K2ConnectSection'
import { listAccountServers, requestEnrollTicket } from '../lib/compute-account'
import { MachinesPart } from './ComputeAccessParts'
import { ApprovalsPanel, NodeAdminPanel } from './ComputeAdminParts'

export const COMPUTE_MANIFEST: SettingEntry[] = [
  { id: 'compute.preview', section: 'compute', label: 'Compute nodes (preview)', description: 'Let this server run jobs on your other machines', keywords: ['compute', 'node', 'nodes', 'preview', 'k2 compute', 'offload', 'build'] },
  { id: 'compute.machines', section: 'compute', label: 'Machines you can use', description: 'Your machines that offer compute; switch one on for this server', keywords: ['compute', 'machine', 'machines', 'use compute', 'cpu', 'ram', 'disk', 'one-click'] },
  { id: 'compute.nodes', section: 'compute', label: 'Nodes of this server', description: 'Machines that run jobs for this server', keywords: ['compute', 'node', 'pause', 'drain', 'resume', 'confirm', 'rename', 'remove'] },
  { id: 'compute.add-node', section: 'compute', label: 'Add a machine with an enroll code', description: 'Make an enroll code for another machine', keywords: ['enroll', 'code', 'install', 'pair', 'add node'] },
  { id: 'compute.grants', section: 'compute', label: 'Compute grants', description: 'Which workspaces may use each node', keywords: ['grant', 'revoke', 'workspace', 'limits'] },
  { id: 'compute.jobs', section: 'compute', label: 'Recent compute jobs', description: 'Jobs and audit lines', keywords: ['jobs', 'events', 'audit', 'log'] },
  { id: 'compute.this-computer', section: 'compute', label: 'Make this computer a compute node', description: 'Let agents on one of your servers run programs here', keywords: ['this computer', 'k2node', 'node', 'enable', 'policy', 'cpu', 'memory'] },
]

const POLL_MS = 5000

type LoadState = 'loading' | 'ready' | 'disabled' | 'error'

function isHostSwitched(err: unknown): boolean {
  return err instanceof Error && err.name === 'HostSwitchedError'
}

function nowSec(): number {
  return Math.floor(Date.now() / 1000)
}

/** "123456" → "123 456"; anything else unchanged. */
function displaySas(s: string | null | undefined): string {
  const t = (s ?? '').replace(/\s+/g, '')
  return /^\d{6}$/.test(t) ? `${t.slice(0, 3)} ${t.slice(3)}` : (s ?? '')
}

function inFuture(atSec: number): string {
  const mins = Math.max(0, Math.round((atSec - nowSec()) / 60))
  return mins <= 0 ? 'expires in under a minute' : `expires in ${mins} min`
}

// ── Main ─────────────────────────────────────────────────────────────

export function ComputeSection(): React.JSX.Element {
  const computePreview = useSettingsStore((s) => s.computePreview)
  const setComputePreview = useSettingsStore((s) => s.setComputePreview)
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const showLocal = webFeatures.computeLocal

  const [nodesList, setNodesList] = useState<ComputeNodesList | null>(null)
  const [controllerState, setControllerState] = useState<LoadState>('loading')
  const [controllerError, setControllerError] = useState<string | null>(null)
  const [grants, setGrants] = useState<ComputeGrant[]>([])
  const [jobs, setJobs] = useState<ComputeJob[]>([])
  const [events, setEvents] = useState<ComputeEvent[]>([])
  const [sideErrors, setSideErrors] = useState<{ grants?: string; jobs?: string; events?: string }>({})
  const [local, setLocal] = useState<LocalStatus | null>(null)
  const [localState, setLocalState] = useState<LoadState>('loading')
  const [localError, setLocalError] = useState<string | null>(null)

  const mounted = useRef(true)
  const inflight = useRef(false)

  const refresh = useCallback(async (): Promise<void> => {
    if (inflight.current) return
    inflight.current = true
    try {
      const controllerTask = (async () => {
        try {
          const list = await fetchComputeNodes()
          if (!mounted.current) return
          setNodesList(list)
          setControllerState('ready')
          setControllerError(null)
        } catch (err) {
          if (!mounted.current || isHostSwitched(err)) return
          if (isComputeDisabled(err)) {
            setControllerState('disabled')
            setNodesList(null)
          } else {
            setControllerState('error')
            setControllerError(computeErrorMessage(err))
          }
          return
        }
        const [g, j, e] = await Promise.allSettled([
          fetchComputeGrants(),
          fetchComputeJobs(20),
          fetchComputeEvents(30),
        ])
        if (!mounted.current) return
        const errs: { grants?: string; jobs?: string; events?: string } = {}
        if (g.status === 'fulfilled') setGrants(g.value)
        else if (!isHostSwitched(g.reason)) errs.grants = computeErrorMessage(g.reason)
        if (j.status === 'fulfilled') setJobs(j.value)
        else if (!isHostSwitched(j.reason)) errs.jobs = computeErrorMessage(j.reason)
        if (e.status === 'fulfilled') setEvents(e.value)
        else if (!isHostSwitched(e.reason)) errs.events = computeErrorMessage(e.reason)
        setSideErrors(errs)
      })()
      const localTask = showLocal
        ? (async () => {
            try {
              const st = await fetchLocalStatus()
              if (!mounted.current) return
              setLocal(st)
              setLocalState('ready')
              setLocalError(null)
            } catch (err) {
              if (!mounted.current) return
              if (isComputeDisabled(err)) {
                setLocalState('disabled')
                setLocal(null)
              } else {
                setLocalState('error')
                setLocalError(computeErrorMessage(err))
              }
            }
          })()
        : Promise.resolve()
      await Promise.all([controllerTask, localTask])
    } finally {
      inflight.current = false
    }
  }, [showLocal])

  // Re-read on open and whenever the window switches servers; poll every
  // ~5 s while the page is visible. Stops on unmount.
  useEffect(() => {
    mounted.current = true
    setControllerState('loading')
    setNodesList(null)
    setGrants([])
    setJobs([])
    setEvents([])
    void refresh()
    const id = setInterval(() => {
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return
      void refresh()
    }, POLL_MS)
    return () => {
      mounted.current = false
      clearInterval(id)
    }
  }, [refresh, activeHost])

  const togglePreview = async (next: boolean): Promise<void> => {
    await setComputePreview(next)
    void refresh()
  }

  return (
    <div className="p-6 max-w-4xl space-y-6" data-testid="compute-section">
      <div>
        <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Compute</h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          Let agents on a K2 server run builds and tests on your other machines. Preview.
        </p>
      </div>

      <SettingsGroup title="Compute nodes (preview)">
        <div className="flex items-start gap-3 py-2" data-settings-id="compute.preview">
          <Toggle
            checked={computePreview || controllerState === 'ready'}
            onChange={(v) => void togglePreview(v)}
            aria-label="Compute nodes (preview)"
          />
          <div className="flex-1 min-w-0">
            <div className="text-xs text-[var(--color-text-primary)]">Compute nodes (preview)</div>
            <p className="text-[11px] text-[var(--color-text-muted)] mt-0.5">
              When on, this server can pair with your other machines and agents you allow can run
              programs on them. Only the server&apos;s owner can turn this on.
            </p>
            {computePreview && controllerState === 'disabled' ? (
              <p className="text-[11px] text-[var(--color-status-warn-amber-soft)] mt-1">
                The server still has compute nodes off. Only its owner can turn them on.
              </p>
            ) : null}
          </div>
        </div>
      </SettingsGroup>

      {controllerState === 'disabled' || !nodesList ? null : (
        <MachinesPart list={nodesList} onChanged={refresh} />
      )}

      {controllerState === 'disabled' ? null : (
        <ControllerPart
          state={controllerState}
          error={controllerError}
          list={nodesList}
          grants={grants}
          jobs={jobs}
          events={events}
          sideErrors={sideErrors}
          onChanged={refresh}
        />
      )}

      {showLocal ? (
        <ThisComputerPart
          state={localState}
          error={localError}
          status={local}
          controllerLabel={nodesList?.controller ?? null}
          onChanged={refresh}
        />
      ) : null}
    </div>
  )
}

// ── Shared bits ──────────────────────────────────────────────────────

const DOT_COLOR: Record<NodeDot, string> = {
  online: 'var(--color-status-ok)',
  offline: 'var(--color-text-muted)',
  pending: 'var(--color-status-warn)',
  paused: 'var(--color-status-warn)',
  draining: 'var(--color-status-warn)',
}

function StatusDot({ dot }: { dot: NodeDot }): React.JSX.Element {
  return (
    <span className="inline-flex items-center gap-1.5 text-[10px] text-[var(--color-text-secondary)]">
      <span aria-hidden style={{ width: 8, height: 8, backgroundColor: DOT_COLOR[dot], display: 'inline-block' }} />
      <span data-testid="node-dot">{dot}</span>
    </span>
  )
}

function CopyBox({ text, label }: { text: string; label?: string }): React.JSX.Element {
  const [copied, setCopied] = useState(false)
  const copy = async (): Promise<void> => {
    try {
      await navigator.clipboard.writeText(text)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      setCopied(false)
    }
  }
  return (
    <div className="space-y-1">
      {label ? <div className="text-[10px] text-[var(--color-text-muted)]">{label}</div> : null}
      <div className="flex items-start gap-2">
        <code className="flex-1 min-w-0 break-all text-[11px] bg-[var(--color-bg)] border border-[var(--color-border)] px-2 py-1 select-all">
          {text}
        </code>
        <Button onClick={() => void copy()} aria-label={label ? `Copy ${label}` : 'Copy'}>
          {copied ? 'Copied' : 'Copy'}
        </Button>
      </div>
    </div>
  )
}

/** Run one gesture: busy flag, readable error, then re-read truth. */
function useAction(onChanged: () => Promise<void>): {
  busy: boolean
  error: string | null
  run: <T>(fn: () => Promise<T>) => Promise<T | undefined>
  setError: (e: string | null) => void
} {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const run = useCallback(
    async <T,>(fn: () => Promise<T>): Promise<T | undefined> => {
      setBusy(true)
      setError(null)
      try {
        const out = await fn()
        return out
      } catch (err) {
        setError(computeErrorMessage(err))
        return undefined
      } finally {
        setBusy(false)
        void onChanged()
      }
    },
    [onChanged],
  )
  return { busy, error, run, setError }
}

// ── B. Nodes of this server ──────────────────────────────────────────

function ControllerPart({
  state,
  error,
  list,
  grants,
  jobs,
  events,
  sideErrors,
  onChanged,
}: {
  state: LoadState
  error: string | null
  list: ComputeNodesList | null
  grants: ComputeGrant[]
  jobs: ComputeJob[]
  events: ComputeEvent[]
  sideErrors: { grants?: string; jobs?: string; events?: string }
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const nodes = (list?.nodes ?? []).filter((n) => n.state !== 'revoked')

  return (
    <div className="space-y-6">
      <SettingsGroup title={list?.controller ? `Node details: ${list.controller}` : 'Node details'}>
        <div data-settings-id="compute.nodes" className="space-y-2 py-1">
          {state === 'loading' && !list ? (
            <p className="text-[11px] text-[var(--color-text-muted)]">Loading…</p>
          ) : null}
          {state === 'error' && error ? <Callout tone="error">{error}</Callout> : null}
          {list && nodes.length > 0 ? <ApprovalsPanel onChanged={() => void onChanged()} /> : null}
          {list && nodes.length === 0 ? (
            <p className="text-[11px] text-[var(--color-text-muted)]">
              No nodes yet. Add one below, or make another computer a node from its own K2.
            </p>
          ) : null}
          {(list?.waitingCodes ?? []).map((c) => (
            <div key={c.name} className="text-[11px] text-[var(--color-text-secondary)]">
              Waiting for <span className="font-medium">{c.name}</span> to enroll ({inFuture(c.expiresAt)})
            </div>
          ))}
          {nodes.map((n) => (
            <NodeCard
              key={n.id}
              node={n}
              grants={grants.filter((g) => g.nodeId === n.id)}
              onChanged={onChanged}
            />
          ))}
          {sideErrors.grants ? <Callout tone="warn">Grants: {sideErrors.grants}</Callout> : null}
        </div>
      </SettingsGroup>

      {list ? <AddNodeForm onChanged={onChanged} /> : null}

      {list ? (
        <SettingsGroup title="Recent jobs">
          <div data-settings-id="compute.jobs" className="space-y-1 py-1">
            {sideErrors.jobs ? <Callout tone="warn">{sideErrors.jobs}</Callout> : null}
            {jobs.length === 0 && !sideErrors.jobs ? (
              <p className="text-[11px] text-[var(--color-text-muted)]">No jobs yet.</p>
            ) : null}
            {jobs.map((j) => (
              <div key={j.id} className="text-[11px] flex flex-wrap gap-x-3 gap-y-0.5 border-b border-[var(--color-border)] py-1">
                <span className="font-mono text-[var(--color-text-primary)]">{j.short}</span>
                <span className="text-[var(--color-text-secondary)]">{j.state}{j.reason ? ` · ${j.reason}` : ''}</span>
                {j.exitCode !== null && j.exitCode !== undefined ? (
                  <span className="text-[var(--color-text-muted)]">exit {j.exitCode}</span>
                ) : null}
                <span className="text-[var(--color-text-muted)]">{j.node ?? '—'} · {j.workspace}</span>
                <span className="font-mono text-[var(--color-text-secondary)] truncate max-w-full">{j.argv.join(' ')}</span>
                {jobNeeds(j.needs) ? (
                  <span className="text-[var(--color-text-muted)]" data-testid="compute-job-needs">needs {jobNeeds(j.needs)}</span>
                ) : null}
                <span className="text-[var(--color-text-muted)]">
                  {formatRelativeTime(j.createdAt, nowSec())}
                  {j.endedAt ? ` · ended ${formatRelativeTime(j.endedAt, nowSec())}` : ''}
                </span>
              </div>
            ))}
          </div>
        </SettingsGroup>
      ) : null}

      {list ? (
        <SettingsGroup title="Audit">
          <div className="space-y-1 py-1">
            {sideErrors.events ? <Callout tone="warn">{sideErrors.events}</Callout> : null}
            {events.length === 0 && !sideErrors.events ? (
              <p className="text-[11px] text-[var(--color-text-muted)]">Nothing yet.</p>
            ) : null}
            {events.map((e) => (
              <div key={e.id} className="text-[11px] flex flex-wrap gap-x-3 border-b border-[var(--color-border)] py-1">
                <span className="text-[var(--color-text-muted)]">{formatRelativeTime(e.at, nowSec())}</span>
                <span className="text-[var(--color-text-secondary)]">{e.actor}</span>
                <span className="text-[var(--color-text-primary)]">{e.kind}</span>
                <span className="font-mono text-[var(--color-text-muted)] break-all">{eventDetail(e.detail)}</span>
              </div>
            ))}
          </div>
        </SettingsGroup>
      ) : null}
    </div>
  )
}

/** A job's declared needs, compact: a list, a {kind: [names]} map, or text. */
function jobNeeds(needs: unknown): string {
  if (needs === null || needs === undefined) return ''
  if (typeof needs === 'string') return needs
  if (Array.isArray(needs)) return needs.map((n) => (typeof n === 'string' ? n : JSON.stringify(n))).join(', ')
  if (typeof needs === 'object') {
    const parts: string[] = []
    for (const [k, v] of Object.entries(needs as Record<string, unknown>)) {
      if (v === null || v === undefined || v === false) continue
      if (Array.isArray(v)) {
        if (v.length > 0) parts.push(`${k}: ${v.map((x) => (typeof x === 'string' ? x : JSON.stringify(x))).join(', ')}`)
      } else if (v === true) parts.push(k)
      else parts.push(`${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`)
    }
    return parts.join('; ')
  }
  return String(needs)
}

function eventDetail(d: unknown): string {
  if (d === null || d === undefined) return ''
  if (typeof d === 'string') return d
  try {
    const s = JSON.stringify(d)
    return s === '{}' ? '' : s
  } catch {
    return ''
  }
}

function NodeCard({
  node,
  grants,
  onChanged,
}: {
  node: ComputeNode
  grants: ComputeGrant[]
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const { busy, error, run } = useAction(onChanged)
  const [typedSas, setTypedSas] = useState('')
  const [renaming, setRenaming] = useState(false)
  const [newName, setNewName] = useState(node.name)
  const dot = nodeDot(node)
  const offer = node.offer
  const tools = offer?.tools ? Object.entries(offer.tools) : []
  const features = Array.isArray(offer?.features) ? offer.features : []

  const remove = async (): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Remove ${node.name}?`,
      message:
        'The node stops taking jobs from this server and its queued and running jobs are cancelled. To use it again it has to enroll again.',
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (!ok) return
    await run(() => removeComputeNode(node.name))
  }

  const saveRename = async (): Promise<void> => {
    const name = newName.trim()
    const out = await run(async () => {
      if (!validNodeName(name)) throw new Error('Node names are 1–32 of a-z, 0-9 and -')
      return renameComputeNode(node.name, name)
    })
    if (out) setRenaming(false)
  }

  return (
    <div className="border border-[var(--color-border)] p-3 space-y-2" data-testid={`compute-node-${node.name}`}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <StatusDot dot={dot} />
        <span className="text-xs font-medium text-[var(--color-text-primary)]">{node.name}</span>
        <span className="text-[11px] text-[var(--color-text-muted)]">
          {[node.os, node.arch].filter(Boolean).join(' / ') || '—'}
        </span>
        {offer ? (
          <span className="text-[11px] text-[var(--color-text-secondary)]">
            {formatCores(offer.caps.cpu_millis)} · {formatBytes(offer.caps.mem_bytes)} RAM · {formatBytes(offer.free.disk_free_bytes)} free
            {offer.caps.hard ? '' : ' · soft caps'}
          </span>
        ) : null}
        <span className="text-[11px] text-[var(--color-text-secondary)]">
          {node.running} running · {node.queued} queued
        </span>
        <span className="text-[11px] text-[var(--color-text-muted)]">
          seen {formatRelativeTime(node.lastSeen, nowSec())}
          {node.liveRoute || node.lastRoute ? ` · via ${node.liveRoute || node.lastRoute}` : ''}
        </span>
      </div>
      {offer && !offer.available && offer.unavailable.length > 0 ? (
        <div className="text-[11px] text-[var(--color-status-warn-amber-soft)]">Not taking jobs: {offer.unavailable.join(', ')}</div>
      ) : null}
      {tools.length > 0 || features.length > 0 ? (
        <div className="text-[10px] text-[var(--color-text-muted)] flex flex-wrap gap-x-2">
          {tools.map(([name, ver]) => (
            <span key={name}>
              {name} {ver}
            </span>
          ))}
          {features.length > 0 ? (
            <span data-testid="compute-node-features" className="text-[var(--color-text-secondary)]">
              [{features.join(', ')}]
            </span>
          ) : null}
        </div>
      ) : null}

      {node.state === 'pending' ? (
        <div className="space-y-2">
          <p className="text-[11px] text-[var(--color-text-secondary)]">
            Waiting for your OK. Type the 6-digit code the node shows.
            {node.sas ? (
              <>
                {' '}It should match <span className="font-mono text-[var(--color-text-primary)]">{displaySas(node.sas)}</span>.
              </>
            ) : null}
          </p>
          <div className="flex items-center gap-2">
            <Input
              value={typedSas}
              onChange={(e) => setTypedSas(e.target.value)}
              placeholder="Code on the node"
              aria-label={`Code shown on ${node.name}`}
              className="w-36 font-mono"
            />
            <Button
              variant="accent"
              disabled={busy || typedSas.trim().length === 0}
              onClick={() => void run(() => confirmComputeNode(node.name, typedSas.trim()))}
            >
              Confirm
            </Button>
            <Button variant="quiet" disabled={busy} onClick={() => void remove()}>
              Remove
            </Button>
          </div>
        </div>
      ) : (
        <div className="space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <Button disabled={busy || node.controllerPause === 'paused'} onClick={() => void run(() => pauseComputeNode(node.name, 'pause'))}>
              Pause
            </Button>
            <Button disabled={busy || node.controllerPause === 'draining'} onClick={() => void run(() => pauseComputeNode(node.name, 'drain'))}>
              Drain
            </Button>
            <Button disabled={busy || node.controllerPause === null} onClick={() => void run(() => pauseComputeNode(node.name, 'resume'))}>
              Resume
            </Button>
            {renaming ? (
              <>
                <Input value={newName} onChange={(e) => setNewName(e.target.value)} aria-label={`New name for ${node.name}`} className="w-32" />
                <Button disabled={busy} onClick={() => void saveRename()}>
                  Save
                </Button>
                <Button variant="ghost" onClick={() => setRenaming(false)}>
                  Cancel
                </Button>
              </>
            ) : (
              <Button
                onClick={() => {
                  setNewName(node.name)
                  setRenaming(true)
                }}
              >
                Rename
              </Button>
            )}
            <Button variant="quiet" disabled={busy} onClick={() => void remove()}>
              Remove
            </Button>
          </div>
          {node.controllerPause ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">
              {node.controllerPause === 'paused' ? 'Paused' : 'Draining'} here: this server sends it no new jobs.
            </p>
          ) : null}
          <GrantsBlock node={node} grants={grants} onChanged={onChanged} />
          <NodeAdminPanel node={node.name} />
        </div>
      )}
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

function GrantsBlock({
  node,
  grants,
  onChanged,
}: {
  node: ComputeNode
  grants: ComputeGrant[]
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const { busy, error, run } = useAction(onChanged)
  const [pick, setPick] = useState('')
  const [limits, setLimits] = useState<Record<keyof GrantLimits, string>>({
    maxJobSecs: '',
    maxDiskGb: '',
    maxParallel: '',
    maxQueued: '',
  })
  const [note, setNote] = useState<string | null>(null)

  const grantedIds = new Set(grants.map((g) => g.workspaceId))
  const options = projects
    .filter((p) => !grantedIds.has(p.id))
    .map((p) => ({ value: p.id, label: p.name }))

  const grant = async (): Promise<void> => {
    if (!pick) return
    const body: GrantLimits = {}
    for (const k of Object.keys(limits) as (keyof GrantLimits)[]) {
      const raw = limits[k].trim()
      if (raw) body[k] = Number(raw)
    }
    const out = await run(() => setComputeGrant(node.name, pick, body))
    if (out) {
      setNote(out.note ?? null)
      setPick('')
    }
  }

  return (
    <div className="space-y-1 pt-1" data-settings-id="compute.grants">
      <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">Workspaces that may use it</div>
      {grants.length === 0 ? (
        <p className="text-[11px] text-[var(--color-text-muted)]">None yet: no workspace may use this node.</p>
      ) : null}
      {grants.map((g) => {
        const project = projects.find((p) => p.id === g.workspaceId)
        const reported = project?.agentsCanUseCompute
        const on = reported === undefined ? undefined : reported === 1
        return (
          <div key={g.workspaceId} className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] border-b border-[var(--color-border)] py-1" data-testid={`compute-grant-${g.workspace}`}>
            <span className="text-[var(--color-text-primary)] font-medium">{project?.name ?? g.workspace}</span>
            <span className="text-[var(--color-text-muted)]">
              ≤{Math.round(g.maxJobSecs / 60)} min · {g.maxDiskGb} GB · {g.maxParallel} at once · {g.maxQueued} queued
            </span>
            {/* Read-only: the switch lives on the Agent tab (Rosson 2026-10-11). */}
            <span className="text-[var(--color-text-secondary)]" data-testid="compute-grant-switch">
              Allow compute nodes: {on === undefined ? 'not reported by this server' : on ? 'on' : 'off (set on its Agent tab)'}
            </span>
            <Button variant="quiet" disabled={busy} onClick={() => void run(() => revokeComputeGrant(node.name, g.workspaceId))}>
              Revoke
            </Button>
          </div>
        )
      })}
      <div className="flex flex-wrap items-center gap-2 pt-1">
        <SettingDropdown
          value={pick}
          options={options}
          onChange={(v) => setPick(v)}
          placeholder="Pick a workspace"
          menuAlign="left"
          ariaLabel={`Workspace to grant ${node.name}`}
        />
        {(
          [
            ['maxJobSecs', 'max secs'],
            ['maxDiskGb', 'disk GB'],
            ['maxParallel', 'parallel'],
            ['maxQueued', 'queued'],
          ] as [keyof GrantLimits, string][]
        ).map(([k, ph]) => (
          <Input
            key={k}
            value={limits[k]}
            onChange={(e) => setLimits((l) => ({ ...l, [k]: e.target.value.replace(/[^0-9]/g, '') }))}
            placeholder={ph}
            aria-label={`${ph} for the grant`}
            className="w-20"
          />
        ))}
        <Button variant="accent" disabled={busy || !pick} onClick={() => void grant()}>
          Grant
        </Button>
      </div>
      {note ? <p className="text-[11px] text-[var(--color-text-muted)]">{note}</p> : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

function AddNodeForm({ onChanged }: { onChanged: () => Promise<void> }): React.JSX.Element {
  const { busy, error, run } = useAction(onChanged)
  const [name, setName] = useState('')
  const [code, setCode] = useState<EnrollCode | null>(null)

  const make = async (): Promise<void> => {
    const n = name.trim()
    const out = await run(async () => {
      if (!validNodeName(n)) throw new Error('Node names are 1–32 of a-z, 0-9 and -')
      return mintEnrollCode(n)
    })
    if (out) setCode(out)
  }

  return (
    <SettingsGroup title="Add a machine with an enroll code">
      <div className="space-y-2 py-1" data-settings-id="compute.add-node">
        <p className="text-[11px] text-[var(--color-text-muted)]">
          Name the machine, then run the install command on it. It shows a 6-digit code; confirm that code here.
        </p>
        <div className="flex items-center gap-2">
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="mini-1" aria-label="New node name" className="w-40" />
          <Button variant="accent" disabled={busy || !name.trim()} onClick={() => void make()}>
            Make enroll code
          </Button>
        </div>
        {error ? <Callout tone="error">{error}</Callout> : null}
        {code ? (
          <div className="space-y-2 border border-[var(--color-border)] p-3" data-testid="compute-enroll-code">
            <div className="text-[11px] text-[var(--color-text-secondary)]">
              Enroll code for <span className="font-medium">{code.name}</span> ({inFuture(code.expiresAt)})
            </div>
            <CopyBox label="Enroll code" text={code.enroll} />
            <CopyBox label="Install on Linux" text={code.install.linux} />
            <CopyBox label="Install on macOS" text={code.install.macos} />
            <CopyBox label="Already installed: enroll only" text={code.install.enrollOnly} />
            <p className="text-[11px] text-[var(--color-text-muted)]">
              Then confirm the code the node shows in its card above (or: <span className="font-mono">{code.confirm}</span>).
            </p>
          </div>
        ) : null}
      </div>
    </SettingsGroup>
  )
}

// ── C. This computer ─────────────────────────────────────────────────

function ThisComputerPart({
  state,
  error,
  status,
  controllerLabel,
  onChanged,
}: {
  state: LoadState
  error: string | null
  status: LocalStatus | null
  controllerLabel: string | null
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const [sheetOpen, setSheetOpen] = useState(false)
  const [enableResult, setEnableResult] = useState<LocalEnableResult | null>(null)
  // A one-click pairing installs the node service first, which turns the
  // node "on" before pairing finishes: keep the sheet up until it's done.
  const [flowActive, setFlowActive] = useState(false)
  // The remote-admin toggles under the main one (Rosson 2026-10-24): K2
  // keeps the node updated and healthy unless the owner says no; root
  // actions only when asked. Sent with the enable, in the same root step.
  const [keepUpdated, setKeepUpdated] = useState(true)
  const [allowRoot, setAllowRoot] = useState(false)
  // On = enrolled (or enrolling). An installed but unenrolled node service
  // (the one-click path installs first) still needs the sheet.
  const isOn = !!status && (status.node ? status.node.state !== 'unenrolled' : status.installed)

  // Once the node is installed (and no pairing is mid-way) the sheet's job
  // is done, and so is the admin command it asked for.
  useEffect(() => {
    if (isOn && !flowActive) {
      setSheetOpen(false)
      setEnableResult(null)
    }
  }, [isOn, flowActive])

  return (
    <SettingsGroup title="This computer">
      <div className="space-y-3 py-1" data-settings-id="compute.this-computer" data-testid="compute-this-computer">
        {state === 'loading' && !status ? <p className="text-[11px] text-[var(--color-text-muted)]">Loading…</p> : null}
        {state === 'disabled' ? (
          <Callout tone="info">
            Compute nodes are off in this computer&apos;s own K2. Switch this window to This computer and turn on
            “Compute nodes (preview)” above.
          </Callout>
        ) : null}
        {state === 'error' && error ? <Callout tone="error">{error}</Callout> : null}

        {status && (!isOn || flowActive) ? (
          <div className="space-y-3">
            <div className="flex items-start gap-3">
              <Toggle checked={sheetOpen} onChange={setSheetOpen} aria-label="Make this computer a compute node" />
              <div className="flex-1 min-w-0">
                <div className="text-xs text-[var(--color-text-primary)]">Make this computer a compute node</div>
                <p className="text-[11px] text-[var(--color-text-muted)] mt-0.5">
                  Agents on the server you pick can run programs here as a separate, limited user (k2node). They
                  can&apos;t read your files unless your files are readable by everyone.
                </p>
              </div>
            </div>
            <RemoteAdminToggles
              keepUpdated={keepUpdated}
              allowRoot={allowRoot}
              onKeepUpdated={(on) => {
                setKeepUpdated(on)
                // Off clears the root opt-in: turning policy back on never
                // brings `full` back by itself.
                if (!on) setAllowRoot(false)
              }}
              onAllowRoot={setAllowRoot}
            />
            {sheetOpen ? (
              <EnableSheet
                status={status}
                remoteAdmin={remoteAdminLevel(keepUpdated, allowRoot)}
                onResult={setEnableResult}
                onFlowActive={setFlowActive}
                onChanged={onChanged}
              />
            ) : null}
          </div>
        ) : null}

        {enableResult ? <EnableResultBox result={enableResult} /> : null}
        {status ? <EnrollProgress status={status} /> : null}

        {status && isOn && !flowActive ? (
          <LocalNodePanel status={status} controllerLabel={controllerLabel} onChanged={onChanged} />
        ) : null}
      </div>
    </SettingsGroup>
  )
}

/** The two remote-admin toggles (prd-compute-update-continuity-v1 Q4/Q5):
 *  policy (on by default) and, only while it's on, full (an explicit
 *  opt-in). Presentation only: the daemon applies the level through root. */
export function RemoteAdminToggles({
  keepUpdated,
  allowRoot,
  onKeepUpdated,
  onAllowRoot,
  disabled = false,
}: {
  keepUpdated: boolean
  allowRoot: boolean
  onKeepUpdated: (on: boolean) => void
  onAllowRoot: (on: boolean) => void
  disabled?: boolean
}): React.JSX.Element {
  return (
    <div className="space-y-2 pl-1" data-testid="compute-remote-admin">
      <div className="flex items-start gap-3">
        <Toggle
          checked={keepUpdated}
          disabled={disabled}
          onChange={onKeepUpdated}
          aria-label="Let K2 keep this node updated and healthy (remote admin: policy)"
        />
        <div className="flex-1 min-w-0">
          <div className="text-xs text-[var(--color-text-primary)]">Let K2 keep this node updated and healthy (remote admin: policy)</div>
          <p className="text-[11px] text-[var(--color-text-muted)] mt-0.5">
            Signed k2-node updates and rollbacks, limits (raising them waits for you), pause, and the job wall.
          </p>
        </div>
      </div>
      {keepUpdated ? (
        <div className="flex items-start gap-3">
          <Toggle
            checked={allowRoot}
            disabled={disabled}
            onChange={onAllowRoot}
            aria-label="Also let K2 run admin scripts as root (remote admin: full)"
          />
          <div className="flex-1 min-w-0">
            <div className="text-xs text-[var(--color-text-primary)]">Also let K2 run admin scripts as root (remote admin: full)</div>
            <p className="text-[11px] text-[var(--color-text-muted)] mt-0.5">
              Each root action still waits for your approval (Touch ID or your password).
            </p>
          </div>
        </div>
      ) : null}
    </div>
  )
}

/** The toggles once the node is on: they show the node's level and change
 *  it through the daemon (the same root path as the install). */
function RemoteAdminPart({ status, onChanged }: { status: LocalStatus; onChanged: () => Promise<void> }): React.JSX.Element {
  const { busy, error, run } = useAction(onChanged)
  const [result, setResult] = useState<LocalRemoteAdminResult | null>(null)
  const level = status.remoteAdmin ?? null
  const keepUpdated = level === 'policy' || level === 'full'
  const change = async (to: RemoteAdminLevel): Promise<void> => {
    if (to === level) return
    const out = await run(() => setLocalRemoteAdmin(to))
    if (out) setResult(out)
  }
  return (
    <div className="space-y-2" data-testid="compute-local-remote-admin">
      <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">Remote admin (now: {level ?? 'not set up'})</div>
      <RemoteAdminToggles
        keepUpdated={keepUpdated}
        allowRoot={level === 'full'}
        disabled={busy}
        onKeepUpdated={(on) => void change(on ? 'policy' : 'off')}
        onAllowRoot={(on) => {
          if (keepUpdated) void change(on ? 'full' : 'policy')
        }}
      />
      {!keepUpdated ? (
        <div className="space-y-1" data-testid="compute-cant-update">
          <Callout tone="warn">K2 can&apos;t update this node. Turn the first switch on, or run this once as an admin:</Callout>
          {status.remoteAdminCommand ? <CopyBox label="Admin command" text={status.remoteAdminCommand} /> : null}
        </div>
      ) : null}
      {result?.rootNeeded && result.command ? <CopyBox label="Admin command" text={result.command} /> : null}
      {result?.message ? <p className="text-[11px] text-[var(--color-text-secondary)]">{result.message}</p> : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

function EnableResultBox({ result }: { result: LocalEnableResult }): React.JSX.Element {
  const needsRoot = result.step === 'needs_root' || (!result.step && !!result.rootNeeded)
  return (
    <div className="space-y-2" data-testid="compute-enable-result">
      {needsRoot && result.command ? (
        <>
          <Callout tone="warn">Run this once in a terminal as an admin; then come back. This page updates by itself.</Callout>
          <CopyBox label="Admin command" text={result.command} />
        </>
      ) : null}
      {result.message ? <p className="text-[11px] text-[var(--color-text-secondary)]">{result.message}</p> : null}
      {result.sas ? (
        <p className="text-[11px] text-[var(--color-text-secondary)]">
          Code: <span className="font-mono text-[var(--color-text-primary)]">{displaySas(result.sas)}</span>
        </p>
      ) : null}
    </div>
  )
}

/** The node's answer to the last enroll request (code or ticket). */
function EnrollProgress({ status }: { status: LocalStatus }): React.JSX.Element | null {
  const r = status.enrollResult ?? null
  if (status.enrollRequestPending) {
    return (
      <p className="text-[11px] text-[var(--color-text-muted)]" data-testid="compute-enroll-pending">
        Waiting for the node to answer the enroll request…
      </p>
    )
  }
  if (r && !r.ok) {
    return (
      <Callout tone="error" data-testid="compute-enroll-failed">
        Enrolling{r.name ? ` ${r.name}` : ''} failed ({formatRelativeTime(r.at, nowSec())}): {r.message ?? 'no reason given'}
      </Callout>
    )
  }
  return null
}

type SheetMode = 'code' | 'account'

type AccountPhase =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'signed_out' }
  | { kind: 'list'; servers: string[] }
  | { kind: 'installing'; sub: string }
  | { kind: 'working'; sub: string }

function EnableSheet({
  status,
  remoteAdmin,
  onResult,
  onFlowActive,
  onChanged,
}: {
  status: LocalStatus
  remoteAdmin: RemoteAdminLevel
  onResult: (r: LocalEnableResult) => void
  onFlowActive: (active: boolean) => void
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const homeCheck = status.homeCheck ?? null
  const oneClickAllowed = !isAirgap()
  const { busy, error, run, setError } = useAction(onChanged)
  const [mode, setMode] = useState<SheetMode>('code')
  const [controller, setController] = useState('')
  const [enroll, setEnroll] = useState('')
  const [name, setName] = useState(status.node?.name ?? '')
  // Checked by default only when the home folder is NOT private yet.
  const [privateHome, setPrivateHome] = useState<boolean>(homeCheck ? !homeCheck.private : false)
  const [notice, setNotice] = useState<string | null>(null)
  const [phase, setPhase] = useState<AccountPhase>({ kind: 'idle' })
  const tokenRef = useRef<string | null>(null)
  // One pairing at a time: the install answer and the polled status can
  // both report the node key.
  const finishing = useRef(false)

  const nodeFp = status.node?.node_fp ?? null

  const submitCode = async (): Promise<void> => {
    const out = await run(async () => {
      const c = controller.trim()
      const e = enroll.trim()
      const n = name.trim()
      if (!/^https?:\/\//.test(c)) throw new Error('The server address starts with https://, e.g. https://rosson.k2.dev')
      if (!e) throw new Error('Paste the enroll code from the server (Settings → Compute → Add a node)')
      if (!validNodeName(n)) throw new Error('Node names are 1–32 of a-z, 0-9 and -')
      return enableLocalNode({ controller: c, enroll: e, name: n, privateHome, remoteAdmin })
    })
    if (out) onResult(out)
  }

  const openAccount = async (): Promise<void> => {
    setMode('account')
    setNotice(null)
    setPhase({ kind: 'loading' })
    const token = await getAccountAccessToken()
    tokenRef.current = token
    if (!token) {
      setPhase({ kind: 'signed_out' })
      return
    }
    try {
      const rows = await listAccountServers(token)
      setPhase({ kind: 'list', servers: rows.map((r) => r.label) })
    } catch (err) {
      setError(computeErrorMessage(err))
      setPhase({ kind: 'idle' })
      setMode('code')
    }
  }

  const fallBackToCode = useCallback((sub: string, message: string): void => {
    setMode('code')
    setController(`https://${sub}.k2.dev`)
    setNotice(message)
    setPhase({ kind: 'idle' })
  }, [])

  const finishOneClick = useCallback(
    async (sub: string, fp: string): Promise<void> => {
      if (finishing.current) return
      finishing.current = true
      try {
        setPhase({ kind: 'working', sub })
        const token = tokenRef.current ?? (await getAccountAccessToken())
        if (!token) {
          setPhase({ kind: 'signed_out' })
          return
        }
        const n = name.trim()
        const ticket = await requestEnrollTicket(token, { controllerSub: sub, nodeFp: fp, nodeName: n })
        if (!ticket.ok) {
          if (ticket.unavailable) fallBackToCode(sub, ticket.message)
          else {
            setError(ticket.message)
            setPhase({ kind: 'list', servers: [sub] })
          }
          return
        }
        const out = await run(() =>
          enableLocalNode({ controller: `https://${sub}.k2.dev`, ticket: ticket.ticket, name: n, privateHome, remoteAdmin }),
        )
        if (out) onResult(out)
        setPhase({ kind: 'idle' })
      } finally {
        finishing.current = false
      }
    },
    [name, privateHome, remoteAdmin, run, onResult, setError, fallBackToCode],
  )

  const pickServer = async (sub: string): Promise<void> => {
    const n = name.trim()
    if (!validNodeName(n)) {
      setError('Name this computer first: 1–32 of a-z, 0-9 and -')
      return
    }
    if (nodeFp) {
      await finishOneClick(sub, nodeFp)
      return
    }
    // No node key yet: install the node service first (no credential). The
    // answer carries the key when the service is up (`installed`/`ready`);
    // after `needs_root` the polled status brings it once the admin ran the
    // command.
    setPhase({ kind: 'installing', sub })
    const out = await run(() => enableLocalNode({ name: n, privateHome, remoteAdmin }))
    if (!out) {
      setPhase({ kind: 'idle' })
      return
    }
    onResult(out)
    if ((out.step === 'installed' || out.step === 'ready') && out.nodeFp) {
      await finishOneClick(sub, out.nodeFp)
    }
  }

  // The installing step continues once the polled status shows a node key.
  useEffect(() => {
    if (phase.kind === 'installing' && nodeFp) void finishOneClick(phase.sub, nodeFp)
  }, [phase, nodeFp, finishOneClick])

  const flowActive = phase.kind === 'installing' || phase.kind === 'working'
  useEffect(() => {
    onFlowActive(flowActive)
  }, [flowActive, onFlowActive])
  useEffect(() => () => onFlowActive(false), [onFlowActive])

  return (
    <div className="border border-[var(--color-border)] p-3 space-y-3" data-testid="compute-enable-sheet">
      <div className="space-y-1">
        <div className="text-[11px] font-medium text-[var(--color-text-primary)]">1. Which server may use this computer?</div>
        <div className="flex gap-2">
          <Button variant={mode === 'code' ? 'accent' : 'quiet'} onClick={() => setMode('code')}>
            Enter an enroll code
          </Button>
          {oneClickAllowed ? (
            <Button variant={mode === 'account' ? 'accent' : 'quiet'} onClick={() => void openAccount()}>
              Pick one of your servers
            </Button>
          ) : null}
        </div>
        {notice ? <Callout tone="info">{notice}</Callout> : null}
        <div className="flex flex-wrap items-center gap-2 pt-1">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="this computer's node name, e.g. mini-1"
            aria-label="Node name"
            className="w-64"
          />
        </div>
        {mode === 'code' ? (
          <div className="flex flex-col gap-2 pt-1">
            <Input
              value={controller}
              onChange={(e) => setController(e.target.value)}
              placeholder="https://rosson.k2.dev"
              aria-label="Server address"
              className="w-80"
            />
            <Input
              value={enroll}
              onChange={(e) => setEnroll(e.target.value)}
              placeholder="ABCDE-FGHJK.0123456789abcdef"
              aria-label="Enroll code"
              className="w-80 font-mono"
            />
          </div>
        ) : (
          <div className="space-y-1 pt-1">
            {phase.kind === 'loading' ? <p className="text-[11px] text-[var(--color-text-muted)]">Reading your servers…</p> : null}
            {phase.kind === 'signed_out' ? (
              <p className="text-[11px] text-[var(--color-text-muted)]">
                Sign in to your k2.dev account in Settings → Tunnel first, or use an enroll code.
              </p>
            ) : null}
            {phase.kind === 'list' ? (
              phase.servers.length === 0 ? (
                <p className="text-[11px] text-[var(--color-text-muted)]">No active servers on this account.</p>
              ) : (
                <div className="flex flex-wrap gap-2">
                  {phase.servers.map((sub) => (
                    <Button key={sub} disabled={busy} onClick={() => void pickServer(sub)}>
                      {sub}.k2.dev
                    </Button>
                  ))}
                </div>
              )
            ) : null}
            {phase.kind === 'installing' ? (
              <p className="text-[11px] text-[var(--color-text-muted)]">Installing the node service… then pairing with {phase.sub}.k2.dev.</p>
            ) : null}
            {phase.kind === 'working' ? (
              <p className="text-[11px] text-[var(--color-text-muted)]">Pairing with {phase.sub}.k2.dev…</p>
            ) : null}
          </div>
        )}
      </div>

      <div className="space-y-1">
        <div className="text-[11px] font-medium text-[var(--color-text-primary)]">2. Your home folder</div>
        {homeCheck ? (
          <div className="text-[11px] text-[var(--color-text-secondary)] space-y-1">
            <p>
              {homeCheck.home} is {homeCheck.private ? 'private' : 'readable by other users'} (mode {homeCheck.mode}).
            </p>
            {homeCheck.readable.length > 0 ? (
              <div>
                <div className="text-[var(--color-text-muted)]">Readable by everyone, so jobs could read them:</div>
                <ul className="list-disc ml-5" data-testid="compute-home-readable">
                  {homeCheck.readable.map((p) => (
                    <li key={p} className="font-mono">
                      {p}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}
          </div>
        ) : (
          <p className="text-[11px] text-[var(--color-text-muted)]">This K2 doesn&apos;t report a home folder check.</p>
        )}
        <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)]">
          <SquareCheckbox checked={privateHome} onChange={(e) => setPrivateHome(e.target.checked)} aria-label="Make my home folder private (chmod 700)" />
          Make my home folder private (chmod 700)
        </label>
      </div>

      {mode === 'code' ? (
        <div>
          <Button variant="accent" disabled={busy} onClick={() => void submitCode()}>
            Make this computer a node
          </Button>
        </div>
      ) : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

const AVAILABILITY_OPTIONS = [
  { value: 'always', label: 'Always' },
  { value: 'plugged_in', label: 'When plugged in' },
  { value: 'idle', label: 'When idle' },
  { value: 'idle_and_plugged_in', label: 'When idle and plugged in' },
  { value: 'window', label: 'In a time window' },
]

const OWNER_RETURN_OPTIONS = [
  { value: 'finish', label: 'Let running jobs finish' },
  { value: 'pause_now', label: 'Pause them now' },
]

function LocalNodePanel({
  status,
  controllerLabel,
  onChanged,
}: {
  status: LocalStatus
  controllerLabel: string | null
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const { busy, error, run } = useAction(onChanged)
  const [disableResult, setDisableResult] = useState<LocalDisableResult | null>(null)
  const node = status.node
  const control = node?.control ?? (typeof status.control?.state === 'string' ? status.control.state : 'active')
  const dot: NodeDot =
    node?.state === 'pending'
      ? 'pending'
      : control === 'paused' || control === 'stopped'
        ? 'paused'
        : control === 'draining'
          ? 'draining'
          : node?.state === 'online'
            ? 'online'
            : 'offline'
  const sameController =
    !!node?.controller && !!controllerLabel && controllerHost(node.controller) === controllerHost(controllerLabel)

  const stop = async (): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: 'Stop being a compute node?',
      message: 'This computer stops taking jobs and the node service, the k2node user and its files are removed.',
      confirmLabel: 'Stop',
      destructive: true,
    })
    if (!ok) return
    const out = await run(() => disableLocalNode())
    if (out) setDisableResult(out)
  }

  return (
    <div className="space-y-3" data-testid="compute-local-node">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <StatusDot dot={dot} />
        <span className="text-xs text-[var(--color-text-primary)]">{node?.name ?? 'Node service installed'}</span>
        <span className="text-[11px] text-[var(--color-text-secondary)]">{node?.state ?? 'starting'}</span>
        {node?.controller ? <span className="text-[11px] text-[var(--color-text-muted)]">for {node.controller}</span> : null}
        {node?.route ? <span className="text-[11px] text-[var(--color-text-muted)]">via {node.route}</span> : null}
      </div>
      {node?.last_error ? <Callout tone="warn">{node.last_error}</Callout> : null}
      {node?.unavailable && node.unavailable.length > 0 ? (
        <p className="text-[11px] text-[var(--color-status-warn-amber-soft)]">Not taking jobs: {node.unavailable.join(', ')}</p>
      ) : null}

      {node?.sas && node.state === 'pending' ? (
        <div className="flex flex-wrap items-center gap-2" data-testid="compute-local-sas">
          <span className="text-[11px] text-[var(--color-text-secondary)]">
            Code: <span className="font-mono text-[var(--color-text-primary)]">{displaySas(node.sas)}</span> — confirm it on{' '}
            {node.controller ?? 'the server'}
          </span>
          {sameController && node.name ? (
            <Button
              variant="accent"
              disabled={busy}
              onClick={() => void run(() => confirmComputeNode(node.name as string, node.sas as string))}
            >
              Confirm
            </Button>
          ) : null}
        </div>
      ) : null}

      <div className="space-y-1">
        <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">Running jobs</div>
        {node?.running && node.running.length > 0 ? (
          <ul className="text-[11px] font-mono text-[var(--color-text-secondary)]">
            {node.running.map((j) => (
              <li key={j}>{j}</li>
            ))}
          </ul>
        ) : (
          <p className="text-[11px] text-[var(--color-text-muted)]">None.</p>
        )}
      </div>

      <div className="space-y-1">
        <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">Your say (now: {control})</div>
        <div className="flex flex-wrap gap-2">
          <Button disabled={busy} onClick={() => void run(() => pauseLocalNode(false))}>
            Pause after current jobs
          </Button>
          <Button disabled={busy} onClick={() => void run(() => pauseLocalNode(true))}>
            Pause now
          </Button>
          <Button disabled={busy} onClick={() => void run(() => drainLocalNode())}>
            Drain
          </Button>
          <Button disabled={busy || control === 'active'} onClick={() => void run(() => resumeLocalNode())}>
            Resume
          </Button>
        </div>
      </div>

      <RemoteAdminPart status={status} onChanged={onChanged} />

      <PolicyEditor policy={status.policy} onChanged={onChanged} />

      {status.packages ? (
        <div className="space-y-1">
          <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">
            Approved system packages{status.packages.manager ? ` (${status.packages.manager})` : ''}
          </div>
          {status.packages.approved.length > 0 ? (
            <p className="text-[11px] font-mono text-[var(--color-text-secondary)]">{status.packages.approved.join(', ')}</p>
          ) : (
            <p className="text-[11px] text-[var(--color-text-muted)]">None.</p>
          )}
          <p className="text-[10px] text-[var(--color-text-muted)]">
            Approve with: <span className="font-mono">sudo k2-node packages allow &lt;name&gt;</span>
          </p>
          {status.packages.installer === false && status.platform !== 'macos' ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">
              This node has no package installer yet (re-run sudo k2-node setup), so approved packages aren&apos;t installed for jobs.
            </p>
          ) : null}
        </div>
      ) : null}

      <div className="space-y-2">
        <Button variant="danger" disabled={busy} onClick={() => void stop()}>
          Stop being a compute node
        </Button>
        {disableResult ? (
          !disableResult.done && disableResult.command ? (
            <div className="space-y-1">
              {disableResult.message ? <p className="text-[11px] text-[var(--color-text-secondary)]">{disableResult.message}</p> : null}
              <CopyBox label="Admin command" text={disableResult.command} />
            </div>
          ) : (
            <p className="text-[11px] text-[var(--color-text-secondary)]">{disableResult.message ?? 'Done.'}</p>
          )
        ) : null}
      </div>
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

function policyString(v: unknown): string {
  if (v === null || v === undefined) return ''
  return String(v)
}

function PolicyEditor({
  policy,
  onChanged,
}: {
  policy: Record<string, unknown> | null
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const { busy, error, run } = useAction(onChanged)
  const base = useMemo<Record<PolicyKey, string>>(
    () => ({
      cpu_cap_percent: policyString(policy?.cpu_cap_percent) || '100',
      mem_cap_gb: policyString(policy?.mem_cap_gb),
      disk_budget_gb: policyString(policy?.disk_budget_gb),
      availability: policyString(policy?.availability) || 'always',
      on_owner_return: policyString(policy?.on_owner_return) || 'finish',
      // The node's default is on.
      repo_toolchains: policy?.repo_toolchains === false ? 'false' : 'true',
    }),
    [policy],
  )
  // The owner's edits over the node's truth. The draft is derived, never
  // synced from `base` in an effect: a mount effect still pending when the
  // first click lands (a just-shown panel) ran after that click and put the
  // old value back, so Save sent nothing.
  const [edits, setEdits] = useState<Partial<Record<PolicyKey, string>>>({})
  const dirty = Object.keys(edits).length > 0
  const draft = useMemo<Record<PolicyKey, string>>(() => ({ ...base, ...edits }), [base, edits])

  const edit = (k: PolicyKey, v: string): void => {
    setEdits((e) => ({ ...e, [k]: v }))
  }

  const save = async (): Promise<void> => {
    const set: Partial<Record<PolicyKey, string | number | boolean>> = {}
    for (const k of Object.keys(draft) as PolicyKey[]) {
      if (draft[k] === base[k] || draft[k] === '') continue
      if (k === 'repo_toolchains') set[k] = draft[k] === 'true'
      else if (k === 'availability' || k === 'on_owner_return') set[k] = draft[k]
      else set[k] = Number(draft[k])
    }
    if (Object.keys(set).length === 0) {
      setEdits({})
      return
    }
    const out = await run(() => setLocalPolicy(set))
    if (out) setEdits({})
  }

  return (
    <div className="space-y-2">
      <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">How much of this computer</div>
      <label className="flex items-center gap-3 text-[11px] text-[var(--color-text-secondary)]">
        <span className="w-28">CPU cap</span>
        <input
          type="range"
          min={1}
          max={100}
          value={Number(draft.cpu_cap_percent) || 100}
          onChange={(e) => edit('cpu_cap_percent', e.target.value)}
          aria-label="CPU cap percent"
          className="flex-1 max-w-xs"
        />
        <span className="w-10 text-right">{draft.cpu_cap_percent}%</span>
      </label>
      <label className="flex items-center gap-3 text-[11px] text-[var(--color-text-secondary)]">
        <span className="w-28">Memory cap (GB)</span>
        <Input
          value={draft.mem_cap_gb}
          onChange={(e) => edit('mem_cap_gb', e.target.value.replace(/[^0-9]/g, ''))}
          aria-label="Memory cap GB"
          className="w-20"
        />
      </label>
      <label className="flex items-center gap-3 text-[11px] text-[var(--color-text-secondary)]">
        <span className="w-28">Disk budget (GB)</span>
        <Input
          value={draft.disk_budget_gb}
          onChange={(e) => edit('disk_budget_gb', e.target.value.replace(/[^0-9]/g, ''))}
          aria-label="Disk budget GB"
          className="w-20"
        />
      </label>
      <div className="flex items-center gap-3 text-[11px] text-[var(--color-text-secondary)]">
        <span className="w-28">Available</span>
        <SettingDropdown
          value={draft.availability}
          options={AVAILABILITY_OPTIONS}
          onChange={(v) => edit('availability', v)}
          menuAlign="left"
          ariaLabel="Availability"
        />
      </div>
      <div className="flex items-start gap-3 text-[11px] text-[var(--color-text-secondary)]">
        <span className="w-28 pt-0.5">Repo toolchains</span>
        <Toggle
          checked={draft.repo_toolchains === 'true'}
          onChange={(v) => edit('repo_toolchains', v ? 'true' : 'false')}
          aria-label="Install the toolchains a job's repo declares"
        />
        <span className="flex-1 min-w-0 text-[var(--color-text-muted)]">
          Install the toolchains a job&apos;s repo declares (mise.toml, .tool-versions, .nvmrc, rust-toolchain.toml)
        </span>
      </div>
      <div className="flex items-center gap-3 text-[11px] text-[var(--color-text-secondary)]">
        <span className="w-28">When I come back</span>
        <SettingDropdown
          value={draft.on_owner_return}
          options={OWNER_RETURN_OPTIONS}
          onChange={(v) => edit('on_owner_return', v)}
          menuAlign="left"
          ariaLabel="When I come back"
        />
      </div>
      <div className="flex gap-2">
        <Button variant="accent" disabled={busy || !dirty} onClick={() => void save()}>
          Save limits
        </Button>
        {dirty ? (
          <Button
            variant="ghost"
            onClick={() => setEdits({})}
          >
            Undo
          </Button>
        ) : null}
      </div>
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}
