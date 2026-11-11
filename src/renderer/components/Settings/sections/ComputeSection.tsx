// Settings → Compute (prd-k2-compute-nodes-v1, preview).
//
// Daemon-first: every fact on this page is read from `/cli/compute/*` and
// every button is one owner gesture posted back. Nothing here schedules,
// sweeps or decides; the polling below only re-reads truth while the page is
// open (effects, never render paths).
//
// The page is four tabs, laid out like every other Settings page (header,
// tab strip, card groups of rows):
//   Machines       the preview switch (`computePreview` on the focused server;
//                  while off the compute routes 404 `compute_disabled` and the
//                  page is just the switch), the machines this server can use
//                  (`ComputeAccessParts`), and Add a machine (an enroll code).
//                  Which agents may use compute is NOT set on this page: it's
//                  the Agent tab's "Allow compute nodes" (Rosson 2026-10-11);
//                  the page links there.
//   Node settings  per node: confirm, pause / drain / resume, rename, remove,
//                  the workspaces that may use it, sharing
//                  (`ComputeServersParts`) and health and admin
//                  (`ComputeAdminParts`); the owner's Approvals and package
//                  requests above them.
//   Jobs           recent jobs and the audit lines.
//   This computer  desktop only, this machine's own daemon
//                  (`scopeForHost('local')`): make it a node, the owner's word
//                  over it, and one-click pairing on the same Connect account.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import {
  SettingsGroup,
  SettingDropdown,
  SettingRow,
  SettingSwitchRow,
  SettingsLoadState,
  SETTING_FIELD_WIDTH,
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
} from '../controls/SettingControls'
import { BetaTag, Button, Input, Checkbox, Toggle, Slider } from '@/components/ui'
import { StatusDot, type HealthTone } from './ConnectionsParts'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import { useConnectHostStore } from '@/stores/connect-host'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { webFeatures } from '@/web/features'
import { isAirgap } from '@/lib/airgap'
import { formatRelativeTime } from '@/lib/format-relative-time'
import {
  computeErrorInfo,
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
  tierCountsLine,
  tierTag,
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
import { NodeSharingPanel } from './ComputeServersParts'
import { LocalPackagesSummary, NodePackagesSummary, PackageRequestsPanel } from './ComputePackagesParts'
import { findLocalNode, onComputeSync } from './compute-packages-api'
import { touchIdOrPassword } from '@/lib/desktop-words'
import { ActionError, CopyBox, NodeStatus, SettingsLink, hardwareLine, osLine } from './ComputeBits'

export const COMPUTE_MANIFEST: SettingEntry[] = [
  { id: 'compute.preview', section: 'compute', label: 'Use compute nodes', description: 'Let this server run jobs on your other machines', keywords: ['compute', 'node', 'nodes', 'preview', 'k2 compute', 'offload', 'build'] },
  { id: 'compute.machines', section: 'compute', label: 'Machines', description: 'Your machines that offer compute; switch one on for this server', keywords: ['compute', 'machine', 'machines', 'use compute', 'cpu', 'ram', 'disk', 'one-click'] },
  { id: 'compute.nodes', section: 'compute', label: 'Node settings', description: 'Machines that run jobs for this server', keywords: ['compute', 'node', 'pause', 'drain', 'resume', 'confirm', 'rename', 'remove'] },
  { id: 'compute.add-node', section: 'compute', label: 'Add a machine', description: 'Make an enroll code for another machine', keywords: ['enroll', 'code', 'install', 'pair', 'add node'] },
  { id: 'compute.grants', section: 'compute', label: 'Workspaces that may use a node', description: 'Which workspaces may use each node', keywords: ['grant', 'revoke', 'workspace', 'limits'] },
  { id: 'compute.servers', section: 'compute', label: 'Servers on a node', description: 'Share a node with your other servers or other people’s; change its owner', keywords: ['servers', 'share', 'shared', 'tenant', 'owner', 'make owner', 'limits', 'queue', 'sas'] },
  { id: 'compute.jobs', section: 'compute', label: 'Recent compute jobs', description: 'Jobs and audit lines', keywords: ['jobs', 'events', 'audit', 'log'] },
  { id: 'compute.this-computer', section: 'compute', label: 'Make this computer a compute node', description: 'Let agents on one of your servers run programs here', keywords: ['this computer', 'k2node', 'node', 'enable', 'policy', 'cpu', 'memory'] },
]

const POLL_MS = 5000

type LoadState = 'loading' | 'ready' | 'disabled' | 'error'
type Tab = 'machines' | 'nodes' | 'jobs' | 'local'

const TAB_LABEL: Record<Tab, string> = {
  machines: 'Machines',
  nodes: 'Node settings',
  jobs: 'Jobs',
  local: 'This computer',
}

const NAME_RULE = 'Use 1 to 32 lowercase letters, numbers or dashes, like mini-1.'

function isHostSwitched(err: unknown): boolean {
  return err instanceof Error && err.name === 'HostSwitchedError'
}

function nowSec(): number {
  return Math.floor(Date.now() / 1000)
}

/** The server's own reason for a refusal: its hint, else its code, else the
 *  error's text. Shown quietly under a plain line, never as the line. */
export function reasonOf(err: unknown): string {
  const { code, hint } = computeErrorInfo(err)
  if (hint) return hint
  if (code) return code.replace(/_/g, ' ')
  return err instanceof Error ? err.message : String(err)
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

  const [tab, setTab] = useState<Tab>('machines')
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
  const [previewError, setPreviewError] = useState<string | null>(null)

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
            setControllerError(reasonOf(err))
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
        else if (!isHostSwitched(g.reason)) errs.grants = reasonOf(g.reason)
        if (j.status === 'fulfilled') setJobs(j.value)
        else if (!isHostSwitched(j.reason)) errs.jobs = reasonOf(j.reason)
        if (e.status === 'fulfilled') setEvents(e.value)
        else if (!isHostSwitched(e.reason)) errs.events = reasonOf(e.reason)
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
                setLocalError(reasonOf(err))
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

  // The daemon says something on Compute changed (a package request filed or
  // decided, an admin request, a node confirmed): re-read now, not at the
  // next poll.
  useEffect(() => onComputeSync(() => void refresh()), [refresh])

  // This machine as one of the focused server's nodes, when it is one.
  const localListedNode = useMemo(
    () => findLocalNode(local, nodesList?.nodes ?? [], nodesList?.controller ?? null),
    [local, nodesList],
  )

  const togglePreview = async (next: boolean): Promise<void> => {
    setPreviewError(null)
    try {
      await setComputePreview(next)
    } catch (err) {
      setPreviewError(reasonOf(err))
    }
    void refresh()
  }

  const disabled = controllerState === 'disabled'
  const tabs: Tab[] = disabled
    ? ['machines', ...(showLocal ? (['local'] as Tab[]) : [])]
    : ['machines', 'nodes', 'jobs', ...(showLocal ? (['local'] as Tab[]) : [])]
  const shown: Tab = tabs.includes(tab) ? tab : 'machines'
  const nodes = (nodesList?.nodes ?? []).filter((n) => n.state !== 'revoked')

  /** The controller's load state, as the card that stands in for a list. */
  const controllerHold = (what: string): React.ReactNode =>
    controllerState === 'error' ? (
      <SettingsLoadState
        kind="error"
        title="Couldn’t load your compute nodes"
        detail={controllerError}
        action={<Button onClick={() => void refresh()}>Try again</Button>}
        testId="compute-load-error"
      >
        This server didn’t answer. Your machines and their jobs are as they were; try again in a moment.
      </SettingsLoadState>
    ) : !nodesList ? (
      <SettingsLoadState kind="loading" title={`Loading ${what}…`} testId="compute-loading" />
    ) : null

  return (
    <div className="w-full max-w-[52rem] space-y-5 pb-6" data-testid="compute-section">
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)] flex items-center gap-2">
          Compute <BetaTag />
        </h2>
        <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed max-w-[75ch]">
          Let agents on this server run builds and tests on your other machines.
        </p>
      </header>

      <div role="tablist" aria-label="Compute" className="flex flex-wrap gap-0.5 border-b border-[var(--color-border)] w-full">
        {tabs.map((t) => {
          const active = shown === t
          return (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={active}
              onClick={() => setTab(t)}
              data-k2-focus=""
              data-testid={`compute-tab-${t}`}
              className={`relative px-3 py-2 type-control transition-colors no-drag cursor-pointer border-b-2 -mb-px outline-none ${
                active
                  ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                  : 'border-transparent text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
              }`}
            >
              {TAB_LABEL[t]}
              {t === 'nodes' && nodes.length > 0 ? <span className="text-[var(--color-text-muted)]"> {nodes.length}</span> : null}
            </button>
          )
        })}
      </div>

      <div className="pt-1 space-y-6">
        {shown === 'machines' ? (
          <>
            <SettingsGroup card title="Compute nodes" settingId="compute.preview">
              <SettingSwitchRow
                control={
                  <Toggle
                    checked={computePreview || controllerState === 'ready'}
                    onChange={(v) => void togglePreview(v)}
                    aria-label="Use compute nodes"
                  />
                }
              >
                <div className={SETTING_ROW_LABEL}>Use compute nodes</div>
                <p className={SETTING_ROW_HINT}>
                  This server can pair with your other machines, and agents you allow can run jobs on them. Only the
                  server’s owner can change this.
                </p>
                {computePreview && disabled ? (
                  <p className="type-hint text-[var(--color-status-warn-text)] mt-1" data-testid="compute-server-off">
                    This server still has compute nodes off. Only its owner can turn them on.
                  </p>
                ) : null}
                {previewError ? <ActionError what="Couldn’t change this setting." reason={previewError} /> : null}
              </SettingSwitchRow>
            </SettingsGroup>

            {disabled ? null : nodesList ? (
              <MachinesPart list={nodesList} onChanged={refresh} />
            ) : (
              <SettingsGroup card title="Machines" state={controllerHold('your machines')} testId="compute-machines" />
            )}

            {nodesList ? <AddNodeForm list={nodesList} onChanged={refresh} onManage={() => setTab('nodes')} /> : null}
          </>
        ) : null}

        {shown === 'nodes' ? (
          <ControllerPart
            hold={controllerHold('your nodes')}
            list={nodesList}
            nodes={nodes}
            grants={grants}
            sideErrors={sideErrors}
            onChanged={refresh}
            onAdd={() => setTab('machines')}
          />
        ) : null}

        {shown === 'jobs' ? (
          <JobsPart hold={controllerHold('recent jobs')} list={nodesList} jobs={jobs} events={events} sideErrors={sideErrors} onRetry={refresh} />
        ) : null}

        {shown === 'local' && showLocal ? (
          <ThisComputerPart
            state={localState}
            error={localError}
            status={local}
            controllerLabel={nodesList?.controller ?? null}
            listedNode={localListedNode}
            onChanged={refresh}
          />
        ) : null}
      </div>
    </div>
  )
}

// ── Shared bits ──────────────────────────────────────────────────────

type ActionFailure = { what: string; reason: string }

/** Run one gesture: busy flag, a plain error line, then re-read truth. */
function useAction(onChanged: () => Promise<void>): {
  busy: boolean
  failure: ActionFailure | null
  run: <T>(what: string, fn: () => Promise<T>) => Promise<T | undefined>
  fail: (what: string, reason: string) => void
  clear: () => void
} {
  const [busy, setBusy] = useState(false)
  const [failure, setFailure] = useState<ActionFailure | null>(null)
  const run = useCallback(
    async <T,>(what: string, fn: () => Promise<T>): Promise<T | undefined> => {
      setBusy(true)
      setFailure(null)
      try {
        const out = await fn()
        return out
      } catch (err) {
        setFailure({ what, reason: reasonOf(err) })
        return undefined
      } finally {
        setBusy(false)
        void onChanged()
      }
    },
    [onChanged],
  )
  return {
    busy,
    failure,
    run,
    fail: (what, reason) => setFailure({ what, reason }),
    clear: () => setFailure(null),
  }
}

function Failure({ failure }: { failure: ActionFailure | null }): React.JSX.Element | null {
  if (!failure) return null
  return <ActionError what={failure.what} reason={failure.reason} />
}

/** A quiet line inside a card, for an empty list. */
function QuietRow({ title, children, testId }: { title: string; children?: React.ReactNode; testId?: string }): React.JSX.Element {
  return (
    <div className="px-4 py-3" data-testid={testId}>
      <p className={SETTING_ROW_LABEL}>{title}</p>
      {children ? <p className={SETTING_ROW_HINT}>{children}</p> : null}
    </div>
  )
}

// ── Node settings ────────────────────────────────────────────────────

function ControllerPart({
  hold,
  list,
  nodes,
  grants,
  sideErrors,
  onChanged,
  onAdd,
}: {
  hold: React.ReactNode
  list: ComputeNodesList | null
  nodes: ComputeNode[]
  grants: ComputeGrant[]
  sideErrors: { grants?: string; jobs?: string; events?: string }
  onChanged: () => Promise<void>
  onAdd: () => void
}): React.JSX.Element {
  if (hold || !list) return <SettingsGroup card title="Node settings" state={hold} />
  if (nodes.length === 0) {
    return (
      <SettingsGroup
        card
        title="Node settings"
        settingId="compute.nodes"
        state={
          <SettingsLoadState kind="empty" title="No nodes yet" action={<Button onClick={onAdd}>Add a machine</Button>} testId="compute-nodes-empty">
            Add a machine with an enroll code, or turn on Make this computer a compute node on the other machine.
          </SettingsLoadState>
        }
      />
    )
  }
  return (
    <div className="space-y-6" data-settings-id="compute.nodes">
      <SettingsGroup card title="Requests" hint="What agents asked to change on your nodes. Nothing runs until you say yes.">
        <div className="px-4 py-3">
          <ApprovalsPanel onChanged={() => void onChanged()} />
        </div>
        <div className="px-4 py-3">
          <PackageRequestsPanel onChanged={() => void onChanged()} />
        </div>
      </SettingsGroup>
      {sideErrors.grants ? (
        <SettingsLoadState kind="error" title="Couldn’t load which workspaces may use each node" detail={sideErrors.grants} action={<Button onClick={() => void onChanged()}>Try again</Button>}>
          The nodes below are fine; only the workspace lists didn’t load.
        </SettingsLoadState>
      ) : null}
      {nodes.map((n) => (
        <NodeCard key={n.id} node={n} grants={grants.filter((g) => g.nodeId === n.id)} onChanged={onChanged} />
      ))}
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

/** An audit line's detail in words: "node mini-1 · job b912". */
function eventDetail(d: unknown): string {
  if (d === null || d === undefined) return ''
  if (typeof d === 'string') return d
  if (typeof d === 'object' && !Array.isArray(d)) {
    return Object.entries(d as Record<string, unknown>)
      .filter(([, v]) => v !== null && v !== undefined && v !== '')
      .map(([k, v]) => `${k.replace(/_/g, ' ')} ${typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean' ? String(v) : JSON.stringify(v)}`)
      .join(' · ')
  }
  try {
    return JSON.stringify(d)
  } catch {
    return ''
  }
}

/** "node.confirmed" → "Node confirmed". */
function eventWords(kind: string): string {
  const t = kind.replace(/[._]+/g, ' ').trim()
  return t ? t[0].toUpperCase() + t.slice(1) : kind
}

const JOB_TONE: Record<string, HealthTone> = {
  running: 'ok',
  failed: 'error',
  error: 'error',
  timeout: 'error',
  queued: 'warn',
}

function jobStateWord(state: string): string {
  const t = state.replace(/_/g, ' ')
  return t ? t[0].toUpperCase() + t.slice(1) : state
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
  const { busy, failure, run } = useAction(onChanged)
  const [typedSas, setTypedSas] = useState('')
  const [renaming, setRenaming] = useState(false)
  const [newName, setNewName] = useState(node.name)
  const dot = nodeDot(node)
  const offer = node.offer
  const tools = offer?.tools ? Object.entries(offer.tools) : []
  const features = Array.isArray(offer?.features) ? offer.features : []
  const tiers = tierCountsLine(node.queueTiers)
  const describe = osLine(node.os, node.arch)

  const remove = async (): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Remove ${node.name}?`,
      message:
        'It stops taking jobs from this server, and its queued and running jobs are cancelled. To use it again it has to enroll again.',
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (!ok) return
    await run(`Couldn’t remove ${node.name}.`, () => removeComputeNode(node.name))
  }

  const saveRename = async (): Promise<void> => {
    const name = newName.trim()
    const out = await run(`Couldn’t rename ${node.name}.`, async () => {
      if (!validNodeName(name)) throw new Error(NAME_RULE)
      return renameComputeNode(node.name, name)
    })
    if (out) setRenaming(false)
  }

  return (
    <SettingsGroup card title={node.name} badge={<NodeStatus dot={dot} />} hint={describe || undefined} testId={`compute-node-${node.name}`}>
      {node.state === 'pending' ? (
        <SettingRow
          label="Confirm this machine"
          hint={
            <>
              Type the 6-digit code {node.name} shows.
              {node.sas ? (
                <>
                  {' '}It should match <span className="font-mono text-[var(--color-text-primary)]">{displaySas(node.sas)}</span>.
                </>
              ) : null}
            </>
          }
        >
          <Input
            value={typedSas}
            onChange={(e) => setTypedSas(e.target.value)}
            placeholder="123 456"
            aria-label={`Code shown on ${node.name}`}
            className="w-32 font-mono"
          />
          <Button
            variant="accent"
            disabled={busy || typedSas.trim().length === 0}
            onClick={() => void run(`Couldn’t confirm ${node.name}.`, () => confirmComputeNode(node.name, typedSas.trim()))}
          >
            Confirm
          </Button>
        </SettingRow>
      ) : (
        <>
          <SettingRow
            label="Jobs"
            hint={
              <>
                {node.running} running · {node.queued} queued
                {tiers ? (
                  <>
                    {' · '}
                    <span data-testid="compute-node-tiers">queued: {tiers}</span>
                  </>
                ) : null}
                {' · '}last seen {formatRelativeTime(node.lastSeen, nowSec())}
                {node.controllerPause ? (
                  <span className="block">
                    {node.controllerPause === 'paused' ? 'Paused' : 'Finishing its jobs'} here: this server sends it no new jobs.
                  </span>
                ) : null}
                {offer && !offer.available && offer.unavailable.length > 0 ? (
                  <span className="block text-[var(--color-status-warn-text)]">Not taking jobs: {offer.unavailable.join(', ')}</span>
                ) : null}
              </>
            }
          >
            <Button
              disabled={busy || node.controllerPause === 'paused'}
              title="Send it no new jobs; running ones keep going"
              onClick={() => void run(`Couldn’t pause ${node.name}.`, () => pauseComputeNode(node.name, 'pause'))}
            >
              Pause
            </Button>
            <Button
              disabled={busy || node.controllerPause === 'draining'}
              title="Let running jobs finish, then take no more"
              onClick={() => void run(`Couldn’t drain ${node.name}.`, () => pauseComputeNode(node.name, 'drain'))}
            >
              Drain
            </Button>
            <Button
              disabled={busy || node.controllerPause === null}
              onClick={() => void run(`Couldn’t resume ${node.name}.`, () => pauseComputeNode(node.name, 'resume'))}
            >
              Resume
            </Button>
          </SettingRow>
          {offer || tools.length > 0 || features.length > 0 || node.host || node.packages || node.rebootPending ? (
            <SettingRow
              label="What it offers"
              below={<NodePackagesSummary node={node} />}
              hint={
                <>
                  {offer ? (
                    <span className="block">
                      {hardwareLine(formatCores(offer.caps.cpu_millis), offer.caps.mem_bytes, offer.free.disk_free_bytes)}
                      {offer.caps.hard ? '' : ' · limits not enforced'}
                    </span>
                  ) : null}
                  {tools.length > 0 ? (
                    <span className="block">
                      {tools.map(([name, ver]) => `${name} ${ver}`).join(' · ')}
                    </span>
                  ) : null}
                  {features.length > 0 ? (
                    <span className="block" data-testid="compute-node-features">
                      [{features.join(', ')}]
                    </span>
                  ) : null}
                </>
              }
            >
              {null}
            </SettingRow>
          ) : null}
          <SettingRow
            label="Name"
            hint="Agents use this name to choose the machine."
            below={
              renaming ? (
                <div className="flex flex-wrap items-center gap-2">
                  <Input value={newName} onChange={(e) => setNewName(e.target.value)} aria-label={`New name for ${node.name}`} className={SETTING_FIELD_WIDTH} />
                  <Button variant="accent" disabled={busy} onClick={() => void saveRename()}>
                    Save
                  </Button>
                  <Button variant="ghost" onClick={() => setRenaming(false)}>
                    Cancel
                  </Button>
                </div>
              ) : undefined
            }
          >
            {renaming ? null : (
              <Button
                onClick={() => {
                  setNewName(node.name)
                  setRenaming(true)
                }}
              >
                Rename
              </Button>
            )}
          </SettingRow>
          <GrantsBlock node={node} grants={grants} onChanged={onChanged} />
          <div className="px-4 py-3 empty:hidden">
            <NodeSharingPanel node={node} />
          </div>
          {/* Another server owns a tenant node: its admin is that server's (V52). */}
          {node.role === 'tenant' ? null : (
            <div className="px-4 py-3 empty:hidden">
              <NodeAdminPanel node={node.name} />
            </div>
          )}
        </>
      )}
      <SettingRow
        label="Remove from this server"
        hint="Its queued and running jobs are cancelled. To use it again it has to enroll again."
        below={failure ? <Failure failure={failure} /> : undefined}
      >
        <Button variant="danger-quiet" disabled={busy} onClick={() => void remove()}>
          Remove…
        </Button>
      </SettingRow>
    </SettingsGroup>
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
  const { busy, failure, run } = useAction(onChanged)
  const [pick, setPick] = useState('')
  const [limits, setLimits] = useState<Record<keyof GrantLimits, string>>({
    maxJobSecs: '',
    maxDiskGb: '',
    maxParallel: '',
    maxQueued: '',
  })
  const [note, setNote] = useState<string | null>(null)

  /** One gesture: the grant as it is, with `allowCi` changed (grants/set
   *  resets any limit left out, so the current ones go along). */
  const setAllowCi = async (g: ComputeGrant, allowCi: boolean): Promise<void> => {
    await run(`Couldn’t change CI jobs for ${g.workspace}.`, () =>
      setComputeGrant(node.name, g.workspaceId, {
        maxJobSecs: g.maxJobSecs,
        maxDiskGb: g.maxDiskGb,
        maxParallel: g.maxParallel,
        maxQueued: g.maxQueued,
        allowCi,
      }),
    )
  }

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
    const out = await run(`Couldn’t let that workspace use ${node.name}.`, () => setComputeGrant(node.name, pick, body))
    if (out) {
      setNote(out.note ?? `Done: it may use ${node.name} now.`)
      setPick('')
    }
  }

  return (
    <div className="px-4 py-3 space-y-2" data-settings-id="compute.grants">
      <div>
        <div className={SETTING_ROW_LABEL}>Workspaces that may use it</div>
        <p className={SETTING_ROW_HINT}>
          Each one runs jobs here within these limits. Whether its agent may use compute at all is set on its Agent tab.
        </p>
      </div>
      {grants.length === 0 ? (
        <p className="type-hint text-[var(--color-text-secondary)]">None yet: no workspace may use this node.</p>
      ) : (
        <div className="divide-y divide-[var(--color-border)] border-y border-[var(--color-border)]">
          {grants.map((g) => {
            const project = projects.find((p) => p.id === g.workspaceId)
            const reported = project?.agentsCanUseCompute
            const on = reported === undefined ? undefined : reported === 1
            const name = project?.name ?? g.workspace
            return (
              <div key={g.workspaceId} className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 py-2" data-testid={`compute-grant-${g.workspace}`}>
                <div className="min-w-0">
                  <div className="type-name text-[var(--color-text-primary)]">{name}</div>
                  <div className="type-hint text-[var(--color-text-secondary)]">
                    Up to {Math.round(g.maxJobSecs / 60)} min a job · {g.maxDiskGb} GB disk · {g.maxParallel} at once · {g.maxQueued} queued
                  </div>
                  {/* Read-only: the switch lives on the Agent tab (Rosson 2026-10-11). */}
                  <div className="type-hint text-[var(--color-text-secondary)]" data-testid="compute-grant-switch">
                    {on === undefined ? 'Its Agent tab doesn’t say whether it may use compute' : on ? 'Its Agent tab allows compute' : 'Its Agent tab doesn’t allow compute yet'}
                  </div>
                </div>
                <div className="flex items-center gap-3 flex-shrink-0">
                  {/* Job tiers (0.46.5): CI is a permission, set by the owner or an
                      Admin only; the daemon refuses anyone else and says so. Absent
                      on older daemons: no switch. */}
                  {g.allowCi === undefined ? null : (
                    <span className="inline-flex items-center gap-2" data-testid="compute-grant-ci">
                      <span className="type-hint text-[var(--color-text-secondary)]">CI jobs</span>
                      <Toggle checked={g.allowCi} disabled={busy} onChange={(v) => void setAllowCi(g, v)} aria-label={`Allow CI jobs: ${name}`} />
                    </span>
                  )}
                  <Button variant="quiet" disabled={busy} onClick={() => void run(`Couldn’t remove ${name} from ${node.name}.`, () => revokeComputeGrant(node.name, g.workspaceId))}>
                    Revoke
                  </Button>
                </div>
              </div>
            )
          })}
        </div>
      )}
      <div className="flex flex-wrap items-center gap-2 pt-1">
        <SettingDropdown
          value={pick}
          options={options}
          onChange={(v) => setPick(v)}
          placeholder="Add a workspace"
          menuAlign="left"
          ariaLabel={`Workspace to grant ${node.name}`}
        />
        {(
          [
            ['maxJobSecs', 'Seconds a job'],
            ['maxDiskGb', 'Disk GB'],
            ['maxParallel', 'At once'],
            ['maxQueued', 'Queued'],
          ] as [keyof GrantLimits, string][]
        ).map(([k, ph]) => (
          <Input
            key={k}
            value={limits[k]}
            onChange={(e) => setLimits((l) => ({ ...l, [k]: e.target.value.replace(/[^0-9]/g, '') }))}
            placeholder={ph}
            title={`${ph} (blank keeps the default)`}
            aria-label={`${ph} for the grant`}
            className="w-28"
          />
        ))}
        <Button variant="accent" disabled={busy || !pick} onClick={() => void grant()}>
          Grant
        </Button>
      </div>
      {grants.some((g) => g.allowCi !== undefined) ? (
        <p className="type-hint text-[var(--color-text-secondary)]" data-testid="compute-grant-ci-help">
          CI jobs go ahead of every queue. Give them to CI and release workspaces, not to agents doing normal work.
        </p>
      ) : null}
      {note ? (
        <p role="status" className="type-hint text-[var(--color-text-secondary)]">
          {note}
        </p>
      ) : null}
      <Failure failure={failure} />
    </div>
  )
}

// ── Machines: add one with an enroll code ────────────────────────────

function AddNodeForm({
  list,
  onChanged,
  onManage,
}: {
  list: ComputeNodesList
  onChanged: () => Promise<void>
  onManage: () => void
}): React.JSX.Element {
  const { busy, failure, run } = useAction(onChanged)
  const [name, setName] = useState('')
  const [code, setCode] = useState<EnrollCode | null>(null)
  const [nameError, setNameError] = useState<string | null>(null)

  const make = async (): Promise<void> => {
    const n = name.trim()
    if (!validNodeName(n)) {
      setNameError(NAME_RULE)
      return
    }
    setNameError(null)
    const out = await run('Couldn’t make an enroll code.', () => mintEnrollCode(n))
    if (out) setCode(out)
  }

  // Where the machine the code was made for has got to.
  const joined = code ? list.nodes.find((n) => n.name === code.name && n.state !== 'revoked') ?? null : null
  const waiting = (list.waitingCodes ?? []).filter((c) => !code || c.name !== code.name)

  const reset = (): void => {
    setCode(null)
    setName('')
  }

  return (
    <SettingsGroup
      card
      title="Add a machine"
      hint="Name it and make a code. Run the install command on that machine, then confirm the code it shows."
      settingId="compute.add-node"
    >
      {!code ? (
        <SettingRow
          label="Machine name"
          hint={nameError ? <span className="text-[var(--color-status-error-soft)]">{nameError}</span> : 'Lowercase letters, numbers and dashes. Agents see this name.'}
          below={<Failure failure={failure} />}
        >
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && name.trim()) void make()
            }}
            placeholder="mini-1"
            aria-label="New node name"
            className="w-44"
          />
          <Button variant="accent" disabled={busy || !name.trim()} onClick={() => void make()}>
            Make enroll code
          </Button>
        </SettingRow>
      ) : joined && joined.state === 'active' ? (
        <div className="px-4 py-3 flex items-center justify-between gap-6" role="status" data-testid="compute-enroll-done">
          <div className="min-w-0">
            <p className="type-row text-[var(--color-status-ok-soft)]">{joined.name} is added</p>
            <p className={SETTING_ROW_HINT}>It takes jobs from this server now. Choose which workspaces may use it in Node settings.</p>
          </div>
          <div className="flex items-center gap-2 flex-shrink-0">
            <Button onClick={onManage}>Node settings</Button>
            <Button variant="quiet" onClick={reset}>
              Add another
            </Button>
          </div>
        </div>
      ) : joined ? (
        <div className="px-4 py-3 flex items-center justify-between gap-6" role="status" data-testid="compute-enroll-joined">
          <div className="min-w-0">
            <p className={SETTING_ROW_LABEL}>{joined.name} checked in</p>
            <p className={SETTING_ROW_HINT}>Confirm the code it shows in its row under Machines above.</p>
          </div>
        </div>
      ) : (
        <div className="px-4 py-3 space-y-3" data-testid="compute-enroll-code">
          <div className="flex items-start justify-between gap-6">
            <div className="min-w-0">
              <p className={SETTING_ROW_LABEL}>
                Code for <span className="font-medium">{code.name}</span>
              </p>
              <p className={SETTING_ROW_HINT}>
                Run one of these on {code.name}; the code {inFuture(code.expiresAt)}. When it starts it shows a 6-digit code and
                appears under Machines above.
              </p>
            </div>
            <Button variant="quiet" onClick={reset}>
              Cancel
            </Button>
          </div>
          <CopyBox label="Install on Linux" text={code.install.linux} />
          <CopyBox label="Install on macOS" text={code.install.macos} />
          <CopyBox label="Already installed: enroll only" text={code.install.enrollOnly} />
          <CopyBox label="Enroll code" text={code.enroll} />
        </div>
      )}
      {waiting.map((c) => (
        <div key={c.name} className="px-4 py-3" data-testid={`compute-waiting-${c.name}`}>
          <p className={SETTING_ROW_LABEL}>Waiting for {c.name} to join</p>
          <p className={SETTING_ROW_HINT}>Its enroll code {inFuture(c.expiresAt)}.</p>
        </div>
      ))}
    </SettingsGroup>
  )
}

// ── Jobs ─────────────────────────────────────────────────────────────

function JobsPart({
  hold,
  list,
  jobs,
  events,
  sideErrors,
  onRetry,
}: {
  hold: React.ReactNode
  list: ComputeNodesList | null
  jobs: ComputeJob[]
  events: ComputeEvent[]
  sideErrors: { grants?: string; jobs?: string; events?: string }
  onRetry: () => Promise<void>
}): React.JSX.Element {
  if (hold || !list) return <SettingsGroup card title="Recent jobs" state={hold} />
  const retry = <Button onClick={() => void onRetry()}>Try again</Button>
  return (
    <div className="space-y-6">
      <SettingsGroup
        card
        title="Recent jobs"
        hint="The last jobs agents sent to your machines."
        settingId="compute.jobs"
        state={
          sideErrors.jobs ? (
            <SettingsLoadState kind="error" title="Couldn’t load recent jobs" detail={sideErrors.jobs} action={retry}>
              The jobs themselves aren’t affected; this page just couldn’t read the list.
            </SettingsLoadState>
          ) : jobs.length === 0 && (list.criticalPath ?? []).length === 0 ? (
            <SettingsLoadState kind="empty" title="No jobs yet" testId="compute-jobs-empty">
              When an agent sends a build or test to one of your machines, it shows here.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {(list.criticalPath ?? []).map((c) => (
          <div key={`${c.workspace}/${c.submitter}`} className="px-4 py-3" data-testid="compute-critical-path">
            <p className={SETTING_ROW_LABEL}>
              {c.submitter} has Priority: {c.jobs} {c.jobs === 1 ? 'job' : 'jobs'}
            </p>
            <p className={SETTING_ROW_HINT}>Priority jobs go ahead of normal work on every node.</p>
          </div>
        ))}
        {jobs.map((j) => (
          <div key={j.id} className={`px-4 py-3 flex items-start justify-between gap-6 ${SETTING_LIST_ROW_HOVER}`} data-testid={`compute-job-${j.short}`}>
            <div className="min-w-0">
              <div className="type-row font-mono text-[var(--color-text-primary)] truncate">{j.argv.join(' ')}</div>
              <div className={SETTING_ROW_HINT}>
                {j.node ? `On ${j.node}` : 'Not placed yet'} · for {j.workspace} · {formatRelativeTime(j.createdAt, nowSec())}
                {j.endedAt ? `, ended ${formatRelativeTime(j.endedAt, nowSec())}` : ''}
                {j.exitCode !== null && j.exitCode !== undefined && j.exitCode !== 0 ? ` · exit code ${j.exitCode}` : ''}
                {j.reason && j.reason !== 'exit' ? ` · ${j.reason.replace(/_/g, ' ')}` : ''}
              </div>
              {jobNeeds(j.needs) ? (
                <div className={SETTING_ROW_HINT} data-testid="compute-job-needs">
                  needs {jobNeeds(j.needs)}
                </div>
              ) : null}
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {tierTag(j.tier) ? (
                <span className="type-hint font-medium text-[var(--color-text-primary)]" data-testid="compute-job-tier" title={j.tierAsked ? `Asked for ${j.tierAsked}` : undefined}>
                  {tierTag(j.tier)}
                </span>
              ) : j.tierAsked ? (
                <span className="type-hint text-[var(--color-text-secondary)]" data-testid="compute-job-tier" title={`Asked for ${j.tierAsked}`}>
                  asked {j.tierAsked}
                </span>
              ) : null}
              <span className="inline-flex items-center gap-1.5 type-hint text-[var(--color-text-secondary)] whitespace-nowrap">
                <StatusDot tone={JOB_TONE[j.state] ?? 'neutral'} />
                {jobStateWord(j.state)}
              </span>
            </div>
          </div>
        ))}
      </SettingsGroup>

      <SettingsGroup
        card
        title="Audit"
        hint="Who changed what on your compute nodes."
        state={
          sideErrors.events ? (
            <SettingsLoadState kind="error" title="Couldn’t load the audit lines" detail={sideErrors.events} action={retry}>
              Nothing was lost; this page just couldn’t read them.
            </SettingsLoadState>
          ) : events.length === 0 ? (
            <SettingsLoadState kind="empty" title="Nothing yet" testId="compute-audit-empty">
              Pairing, pausing, removing and granting each leave a line here.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {events.map((e) => (
          <div key={e.id} className={`px-4 py-3 flex items-start justify-between gap-6 ${SETTING_LIST_ROW_HOVER}`}>
            <div className="min-w-0">
              <div className={SETTING_ROW_LABEL}>
                {eventWords(e.kind)} <span className="text-[var(--color-text-secondary)]">by {e.actor}</span>
              </div>
              {eventDetail(e.detail) ? <div className={`${SETTING_ROW_HINT} break-words`}>{eventDetail(e.detail)}</div> : null}
            </div>
            <span className="type-hint text-[var(--color-text-secondary)] whitespace-nowrap flex-shrink-0">{formatRelativeTime(e.at, nowSec())}</span>
          </div>
        ))}
      </SettingsGroup>
    </div>
  )
}

// ── This computer ────────────────────────────────────────────────────

function ThisComputerPart({
  state,
  error,
  status,
  controllerLabel,
  listedNode,
  onChanged,
}: {
  state: LoadState
  error: string | null
  status: LocalStatus | null
  controllerLabel: string | null
  listedNode: ComputeNode | null
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
  // A machine that already has a remote-admin choice (a reinstall) starts
  // from it, so the enable never raises an earlier `off` by itself
  // (security reviews A-L4 / B-L6). Once the owner flips a switch, theirs.
  const earlier = status?.remoteAdmin ?? null
  const touched = useRef(false)
  useEffect(() => {
    if (touched.current || !earlier) return
    setKeepUpdated(earlier !== 'off')
    setAllowRoot(earlier === 'full')
  }, [earlier])
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

  const hold =
    state === 'loading' && !status ? (
      <SettingsLoadState kind="loading" title="Checking this computer…" />
    ) : state === 'disabled' ? (
      <SettingsLoadState kind="empty" title="Compute nodes are off on this computer">
        This computer’s own K2 has them off. Switch this window to This computer, then turn on Use compute nodes on the
        Machines tab.
      </SettingsLoadState>
    ) : state === 'error' && !status ? (
      <SettingsLoadState kind="error" title="Couldn’t check this computer" detail={error} action={<Button onClick={() => void onChanged()}>Try again</Button>}>
        K2 on this computer didn’t answer. Nothing changed here.
      </SettingsLoadState>
    ) : undefined

  return (
    <div data-settings-id="compute.this-computer" data-testid="compute-this-computer">
      <SettingsGroup
        card
        title="This computer"
        hint="Let agents on one of your servers run programs here, as a separate user with its own limits."
        state={hold}
      >
        {status && (!isOn || flowActive) ? (
          <>
            <SettingSwitchRow control={<Toggle checked={sheetOpen} onChange={setSheetOpen} aria-label="Make this computer a compute node" />}>
              <div className={SETTING_ROW_LABEL}>Make this computer a compute node</div>
              <p className={SETTING_ROW_HINT}>
                Jobs run as a separate, limited user. They can’t read your files unless your files are readable by everyone.
              </p>
            </SettingSwitchRow>
            <RemoteAdminToggles
              keepUpdated={keepUpdated}
              allowRoot={allowRoot}
              onKeepUpdated={(on) => {
                touched.current = true
                setKeepUpdated(on)
                // Off clears the root opt-in: turning policy back on never
                // brings `full` back by itself.
                if (!on) setAllowRoot(false)
              }}
              onAllowRoot={(on) => {
                touched.current = true
                setAllowRoot(on)
              }}
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
          </>
        ) : null}

        {enableResult ? <EnableResultBox result={enableResult} /> : null}
        {status ? <EnrollProgress status={status} /> : null}

        {status && isOn && !flowActive ? (
          <LocalNodePanel status={status} controllerLabel={controllerLabel} listedNode={listedNode} onChanged={onChanged} />
        ) : null}
      </SettingsGroup>
    </div>
  )
}

/** The two remote-admin switches (prd-compute-update-continuity-v1 Q4/Q5):
 *  keep it updated (on by default) and, only while that's on, root scripts
 *  (an explicit opt-in). Presentation only: the daemon applies the level
 *  through root. Rows of a card group. */
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
    <div className="divide-y divide-[var(--color-border)]" data-testid="compute-remote-admin">
      <SettingSwitchRow
        control={<Toggle checked={keepUpdated} disabled={disabled} onChange={onKeepUpdated} aria-label="Let K2 keep this node updated and healthy" />}
      >
        <div className={SETTING_ROW_LABEL}>Let K2 keep this node updated and healthy</div>
        <p className={SETTING_ROW_HINT}>
          K2 installs signed node updates, rolls back a bad one, and can pause jobs or lower limits. Raising a limit waits for you.
        </p>
      </SettingSwitchRow>
      {keepUpdated ? (
        <SettingSwitchRow
          control={<Toggle checked={allowRoot} disabled={disabled} onChange={onAllowRoot} aria-label="Also let K2 run admin scripts as root" />}
        >
          <div className={SETTING_ROW_LABEL}>Also let K2 run admin scripts as root</div>
          <p className={SETTING_ROW_HINT}>Each script still waits for your approval ({touchIdOrPassword()}).</p>
        </SettingSwitchRow>
      ) : null}
    </div>
  )
}

const LEVEL_WORD: Record<RemoteAdminLevel, string> = {
  off: 'K2 can’t update this node',
  policy: 'K2 keeps this node updated and healthy',
  full: 'K2 keeps this node updated and may run admin scripts you approve',
}

/** The switches once the node is on: they show the node's level and change
 *  it through the daemon (the same root path as the install). */
function RemoteAdminPart({ status, onChanged }: { status: LocalStatus; onChanged: () => Promise<void> }): React.JSX.Element {
  const { busy, failure, run } = useAction(onChanged)
  const [result, setResult] = useState<LocalRemoteAdminResult | null>(null)
  const level = status.remoteAdmin ?? null
  const keepUpdated = level === 'policy' || level === 'full'
  const change = async (to: RemoteAdminLevel): Promise<void> => {
    if (to === level) return
    const out = await run('Couldn’t change what K2 may do on this node.', () => setLocalRemoteAdmin(to))
    if (out) setResult(out)
  }
  return (
    <div className="divide-y divide-[var(--color-border)]" data-testid="compute-local-remote-admin">
      <RemoteAdminToggles
        keepUpdated={keepUpdated}
        allowRoot={level === 'full'}
        disabled={busy}
        onKeepUpdated={(on) => void change(on ? 'policy' : 'off')}
        onAllowRoot={(on) => {
          if (keepUpdated) void change(on ? 'full' : 'policy')
        }}
      />
      <div className="px-4 py-3 space-y-2">
        <p className="type-hint text-[var(--color-text-secondary)]">Now: {level ? LEVEL_WORD[level] : 'not set up yet'}.</p>
        {!keepUpdated ? (
          <div className="space-y-2" data-testid="compute-cant-update">
            <p className="type-hint text-[var(--color-status-warn-text)]">
              K2 can’t update this node. Turn the first switch on, or run this once in a terminal as an admin:
            </p>
            {status.remoteAdminCommand ? <CopyBox label="Admin command" text={status.remoteAdminCommand} /> : null}
          </div>
        ) : null}
        {result?.rootNeeded && result.command ? <CopyBox label="Admin command" text={result.command} /> : null}
        {result?.message ? (
          <p role="status" className="type-hint text-[var(--color-text-secondary)]">
            {result.message}
          </p>
        ) : null}
        <Failure failure={failure} />
      </div>
    </div>
  )
}

function EnableResultBox({ result }: { result: LocalEnableResult }): React.JSX.Element {
  const needsRoot = result.step === 'needs_root' || (!result.step && !!result.rootNeeded)
  return (
    <div className="px-4 py-3 space-y-2" data-testid="compute-enable-result" role="status">
      {needsRoot && result.command ? (
        <>
          <p className="type-row text-[var(--color-text-primary)]">One more step: run this once as an admin</p>
          <p className={SETTING_ROW_HINT}>Open a terminal, run it, then come back. This page updates by itself.</p>
          <CopyBox label="Admin command" text={result.command} />
        </>
      ) : null}
      {result.message ? <p className="type-hint text-[var(--color-text-secondary)]">{result.message}</p> : null}
      {result.sas ? (
        <p className="type-hint text-[var(--color-text-secondary)]">
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
      <div className="px-4 py-3" data-testid="compute-enroll-pending" role="status">
        <p className="type-hint text-[var(--color-text-secondary)]">Waiting for the node to answer the enroll request…</p>
      </div>
    )
  }
  if (r && !r.ok) {
    return (
      <div className="px-4 py-3" data-testid="compute-enroll-failed" role="alert">
        <p className="type-hint text-[var(--color-status-error-soft)]">
          Joining{r.name ? ` as ${r.name}` : ''} didn’t work ({formatRelativeTime(r.at, nowSec())}). Check the server address and
          code, then try again.
        </p>
        <p className="type-hint text-[var(--color-text-muted)] mt-0.5">The node said: {r.message ?? 'no reason given'}.</p>
      </div>
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
  const { busy, failure, run, fail, clear } = useAction(onChanged)
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
  const JOIN_FAILED = 'Couldn’t make this computer a node.'

  const submitCode = async (): Promise<void> => {
    const out = await run(JOIN_FAILED, async () => {
      const c = controller.trim()
      const e = enroll.trim()
      const n = name.trim()
      if (!/^https?:\/\//.test(c)) throw new Error('The server address starts with https://, like https://alex.k2.dev')
      if (!e) throw new Error('Paste the enroll code from the server’s Compute page (Add a machine)')
      if (!validNodeName(n)) throw new Error(NAME_RULE)
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
      fail('Couldn’t read your servers.', reasonOf(err))
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
            fail(`Couldn’t pair with ${sub}.k2.dev.`, ticket.message)
            setPhase({ kind: 'list', servers: [sub] })
          }
          return
        }
        const out = await run(JOIN_FAILED, () =>
          enableLocalNode({ controller: `https://${sub}.k2.dev`, ticket: ticket.ticket, name: n, privateHome, remoteAdmin }),
        )
        if (out) onResult(out)
        setPhase({ kind: 'idle' })
      } finally {
        finishing.current = false
      }
    },
    [name, privateHome, remoteAdmin, run, onResult, fail, fallBackToCode],
  )

  const pickServer = async (sub: string): Promise<void> => {
    const n = name.trim()
    if (!validNodeName(n)) {
      fail('Name this computer first.', NAME_RULE)
      return
    }
    clear()
    if (nodeFp) {
      await finishOneClick(sub, nodeFp)
      return
    }
    // No node key yet: install the node service first (no credential). The
    // answer carries the key when the service is up (`installed`/`ready`);
    // after `needs_root` the polled status brings it once the admin ran the
    // command.
    setPhase({ kind: 'installing', sub })
    const out = await run(JOIN_FAILED, () => enableLocalNode({ name: n, privateHome, remoteAdmin }))
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
    <div className="divide-y divide-[var(--color-border)]" data-testid="compute-enable-sheet">
      <SettingRow label="Name for this computer" hint="Agents see this name when they pick a machine." field>
        <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="mini-1" aria-label="Node name" className="w-full" />
      </SettingRow>
      <SettingRow
        stack
        label="Which server may use it?"
        hint={mode === 'code' ? 'Paste the server’s address and the enroll code from its Compute page.' : 'Pick one of the servers on your k2.dev account.'}
        below={
          <div className="space-y-2">
            {notice ? <p className="type-hint text-[var(--color-text-secondary)]">{notice}</p> : null}
            {mode === 'code' ? (
              <div className="flex flex-col gap-2">
                <Input value={controller} onChange={(e) => setController(e.target.value)} placeholder="https://alex.k2.dev" aria-label="Server address" className="w-80 max-w-full" />
                <Input value={enroll} onChange={(e) => setEnroll(e.target.value)} placeholder="Enroll code" aria-label="Enroll code" className="w-80 max-w-full font-mono" />
              </div>
            ) : (
              <div className="space-y-1">
                {phase.kind === 'loading' ? <p className="type-hint text-[var(--color-text-secondary)]">Reading your servers…</p> : null}
                {phase.kind === 'signed_out' ? (
                  <p className="type-hint text-[var(--color-text-secondary)]">
                    Sign in to your k2.dev account first, or use an enroll code. <SettingsLink to="k2-connect">Open Tunnel</SettingsLink>
                  </p>
                ) : null}
                {phase.kind === 'list' ? (
                  phase.servers.length === 0 ? (
                    <p className="type-hint text-[var(--color-text-secondary)]">No active servers on this account.</p>
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
                  <p className="type-hint text-[var(--color-text-secondary)]" role="status">Installing the node service, then pairing with {phase.sub}.k2.dev…</p>
                ) : null}
                {phase.kind === 'working' ? <p className="type-hint text-[var(--color-text-secondary)]" role="status">Pairing with {phase.sub}.k2.dev…</p> : null}
              </div>
            )}
          </div>
        }
      >
        <Button variant={mode === 'code' ? 'accent' : 'quiet'} aria-pressed={mode === 'code'} onClick={() => setMode('code')}>
          Enter an enroll code
        </Button>
        {oneClickAllowed ? (
          <Button variant={mode === 'account' ? 'accent' : 'quiet'} aria-pressed={mode === 'account'} onClick={() => void openAccount()}>
            Pick one of your servers
          </Button>
        ) : null}
      </SettingRow>
      <SettingRow
        label="Your home folder"
        hint={
          homeCheck ? (
            <>
              {homeCheck.home} is {homeCheck.private ? 'private' : 'readable by other users'}.
              {homeCheck.readable.length > 0 ? (
                <span className="block">
                  Readable by everyone, so jobs could read them:
                  <span className="block font-mono" data-testid="compute-home-readable">
                    {homeCheck.readable.map((p) => (
                      <span key={p} className="block">
                        {p}
                      </span>
                    ))}
                  </span>
                </span>
              ) : null}
            </>
          ) : (
            'This K2 doesn’t report whether your home folder is private.'
          )
        }
      >
        <label className="flex items-center gap-2 type-hint text-[var(--color-text-secondary)] cursor-pointer">
          <Checkbox checked={privateHome} onChange={(e) => setPrivateHome(e.target.checked)} aria-label="Make my home folder private (chmod 700)" />
          Make it private
        </label>
      </SettingRow>
      {mode === 'code' || failure ? (
        <div className="px-4 py-3">
          {mode === 'code' ? (
            <div className="flex justify-end">
              <Button variant="accent" disabled={busy} onClick={() => void submitCode()}>
                Make this computer a node
              </Button>
            </div>
          ) : null}
          <Failure failure={failure} />
        </div>
      ) : null}
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

const CONTROL_WORD: Record<string, string> = {
  active: 'Taking jobs',
  paused: 'Paused',
  stopped: 'Paused',
  draining: 'Finishing its jobs, then pausing',
}

function LocalNodePanel({
  status,
  controllerLabel,
  listedNode,
  onChanged,
}: {
  status: LocalStatus
  controllerLabel: string | null
  /** This machine as a node of the focused server, when it is one. */
  listedNode: ComputeNode | null
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const { busy, failure, run } = useAction(onChanged)
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
    const running = node?.running ?? []
    const ok = await useConfirmDialogStore.getState().confirm({
      title: 'Stop being a compute node?',
      message:
        running.length > 0
          ? `${running.length === 1 ? 'This job' : `These ${running.length} jobs`} running here stop now: ${running.join(', ')}. ` +
            'This computer stops taking jobs, and the node service, its user and its files are removed.'
          : 'No jobs are running here. This computer stops taking jobs, and the node service, its user and its files are removed.',
      confirmLabel: 'Stop',
      destructive: true,
    })
    if (!ok) return
    const out = await run('Couldn’t stop this computer being a node.', () => disableLocalNode())
    if (out) setDisableResult(out)
  }

  return (
    <div className="divide-y divide-[var(--color-border)]" data-testid="compute-local-node">
      <div className="px-4 py-3 flex items-start justify-between gap-6">
        <div className="min-w-0">
          <div className="type-name text-[var(--color-text-primary)]">{node?.name ?? 'Node service installed'}</div>
          <div className={SETTING_ROW_HINT}>
            {node?.controller ? `Runs jobs for ${controllerHost(node.controller) || node.controller}` : 'Not paired with a server yet'}
            {node?.route ? ` · connected ${node.route === 'relay' ? 'through the relay' : `by ${node.route}`}` : ''}
          </div>
          {node?.last_error ? <p className="type-hint text-[var(--color-status-warn-text)] mt-1">{node.last_error}</p> : null}
          {node?.unavailable && node.unavailable.length > 0 ? (
            <p className="type-hint text-[var(--color-status-warn-text)] mt-1">Not taking jobs: {node.unavailable.join(', ')}</p>
          ) : null}
        </div>
        <NodeStatus dot={dot} />
      </div>

      {node?.sas && node.state === 'pending' ? (
        <SettingRow
          label="Confirm the code"
          testId="compute-local-sas"
          hint={
            <span>
              Code <span className="font-mono text-[var(--color-text-primary)]">{displaySas(node.sas)}</span>: confirm it on{' '}
              {node.controller ?? 'the server'}.
            </span>
          }
        >
          {sameController && node.name ? (
            <Button
              variant="accent"
              disabled={busy}
              onClick={() => void run('Couldn’t confirm this computer.', () => confirmComputeNode(node.name as string, node.sas as string))}
            >
              Confirm
            </Button>
          ) : null}
        </SettingRow>
      ) : null}

      <SettingRow
        label="Running here"
        hint={
          node?.running && node.running.length > 0 ? (
            <span className="font-mono">{node.running.join(', ')}</span>
          ) : (
            'No jobs are running on this computer.'
          )
        }
      >
        {null}
      </SettingRow>

      <SettingRow label="Your say" hint={`Now: ${CONTROL_WORD[control] ?? control}. Pausing never loses a job; it waits until you resume.`}>
        <Button disabled={busy} onClick={() => void run('Couldn’t pause this computer.', () => pauseLocalNode(false))}>
          Pause after current jobs
        </Button>
        <Button disabled={busy} onClick={() => void run('Couldn’t pause this computer.', () => pauseLocalNode(true))}>
          Pause now
        </Button>
        <Button disabled={busy} onClick={() => void run('Couldn’t drain this computer.', () => drainLocalNode())}>
          Drain
        </Button>
        <Button disabled={busy || control === 'active'} onClick={() => void run('Couldn’t resume this computer.', () => resumeLocalNode())}>
          Resume
        </Button>
      </SettingRow>

      <RemoteAdminPart status={status} onChanged={onChanged} />

      <PolicyEditor policy={status.policy} onChanged={onChanged} />

      {status.packages ? (
        <SettingRow
          label="Approved system packages"
          hint={
            <>
              {status.packages.approved.length > 0 ? (
                <span className="block font-mono">{status.packages.approved.join(', ')}</span>
              ) : (
                <span className="block">None yet.</span>
              )}
              <span className="block">
                Approve one at this computer with <span className="font-mono">sudo k2-node packages allow &lt;name&gt;</span>.
              </span>
              {status.packages.installer === false && status.platform !== 'macos' ? (
                <span className="block">
                  This node has no package installer yet (run <span className="font-mono">sudo k2-node setup</span> again), so approved
                  packages aren’t installed for jobs.
                </span>
              ) : null}
            </>
          }
          below={listedNode ? <LocalPackagesSummary node={listedNode} /> : undefined}
        >
          {null}
        </SettingRow>
      ) : listedNode ? (
        <div className="px-4 py-3 empty:hidden">
          <LocalPackagesSummary node={listedNode} />
        </div>
      ) : null}

      <SettingRow
        label="Stop being a compute node"
        hint="Stops taking jobs and removes the node service, its user and its files from this computer."
        below={
          <>
            {disableResult ? (
              !disableResult.done && disableResult.command ? (
                <div className="space-y-1">
                  {disableResult.message ? <p className="type-hint text-[var(--color-text-secondary)]">{disableResult.message}</p> : null}
                  <CopyBox label="Admin command" text={disableResult.command} />
                </div>
              ) : (
                <p role="status" className="type-hint text-[var(--color-status-ok-soft)]">
                  {disableResult.message ?? 'Done: this computer is no longer a compute node.'}
                </p>
              )
            ) : null}
            <Failure failure={failure} />
          </>
        }
      >
        <Button variant="danger-quiet" disabled={busy} onClick={() => void stop()}>
          Stop being a compute node
        </Button>
      </SettingRow>
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
  const { busy, failure, run } = useAction(onChanged)
  const [saved, setSaved] = useState(false)
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
    setSaved(false)
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
    const out = await run('Couldn’t save the limits.', () => setLocalPolicy(set))
    if (out) {
      setEdits({})
      setSaved(true)
    }
  }

  return (
    <div className="divide-y divide-[var(--color-border)]" data-testid="compute-local-policy">
      <div className="px-4 py-3">
        <div className="type-heading text-[var(--color-text-primary)]">How much of this computer</div>
        <p className={SETTING_ROW_HINT}>Jobs never take more than this. Changes apply to the next job.</p>
      </div>
      <SettingRow label="Processor" hint="The most of the processor jobs may use.">
        <Slider
          min={1}
          max={100}
          value={Number(draft.cpu_cap_percent) || 100}
          onChange={(e) => edit('cpu_cap_percent', e.target.value)}
          aria-label="CPU cap percent"
          className="w-40"
        />
        <span className="type-hint text-[var(--color-text-secondary)] w-10 text-right">{draft.cpu_cap_percent}%</span>
      </SettingRow>
      <SettingRow label="Memory (GB)" hint="Blank means no cap of its own." field>
        <Input value={draft.mem_cap_gb} onChange={(e) => edit('mem_cap_gb', e.target.value.replace(/[^0-9]/g, ''))} aria-label="Memory cap GB" className="w-full" />
      </SettingRow>
      <SettingRow label="Disk (GB)" hint="Space jobs may fill with their files." field>
        <Input value={draft.disk_budget_gb} onChange={(e) => edit('disk_budget_gb', e.target.value.replace(/[^0-9]/g, ''))} aria-label="Disk budget GB" className="w-full" />
      </SettingRow>
      <SettingRow label="Available" hint="When this computer takes jobs." field>
        <SettingDropdown value={draft.availability} options={AVAILABILITY_OPTIONS} onChange={(v) => edit('availability', v)} ariaLabel="Availability" />
      </SettingRow>
      <SettingRow label="When I come back" hint="What happens to running jobs when you start using this computer." field>
        <SettingDropdown value={draft.on_owner_return} options={OWNER_RETURN_OPTIONS} onChange={(v) => edit('on_owner_return', v)} ariaLabel="When I come back" />
      </SettingRow>
      <SettingSwitchRow
        control={
          <Toggle
            checked={draft.repo_toolchains === 'true'}
            onChange={(v) => edit('repo_toolchains', v ? 'true' : 'false')}
            aria-label="Install the toolchains a job's repo declares"
          />
        }
      >
        <div className={SETTING_ROW_LABEL}>Install the tools a job’s project asks for</div>
        <p className={SETTING_ROW_HINT}>Versions named in files like .nvmrc, .tool-versions or rust-toolchain.toml.</p>
      </SettingSwitchRow>
      <div className="px-4 py-3">
        <div className="flex items-center justify-end gap-2">
          {saved && !dirty ? (
            <span role="status" className="type-hint text-[var(--color-status-ok-soft)]">
              Saved
            </span>
          ) : null}
          {dirty ? (
            <Button variant="ghost" onClick={() => setEdits({})}>
              Undo
            </Button>
          ) : null}
          <Button variant="accent" disabled={busy || !dirty} onClick={() => void save()}>
            Save limits
          </Button>
        </div>
        <Failure failure={failure} />
      </div>
    </div>
  )
}
