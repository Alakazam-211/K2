// Settings → Compute, the two everyday sections (Rosson 2026-10-11):
//
//   1. Machines you can use — this server's nodes plus the account's
//      machines with "Make this computer a node" on. Each row's toggle
//      "Use compute from this machine": on = one-click enroll as a node of
//      THIS server (K2 Connect delivers the ticket to that machine); off =
//      drain, then remove (daemon `nodes/retire`).
//   2. Agents that can use compute — one toggle per workspace: on = the
//      workspace switch plus a grant on every machine (daemon
//      `agents/set`); machines added later are granted by the daemon.
//
// Thin client: the daemon owns nodes, grants and the retire sweep; K2
// Connect owns the account's machine list. The page only re-reads while
// open and sends the owner's gestures.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { SettingsGroup } from '../controls/SettingControls'
import { Button, Callout, Input, Toggle } from '@/components/ui'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useProjectsStore } from '@/stores/projects'
import { isAirgap } from '@/lib/airgap'
import {
  computeErrorMessage,
  confirmComputeNode,
  formatBytes,
  pauseComputeNode,
  retireComputeNode,
  setComputeAgent,
  type ComputeAgentRow,
  type ComputeAgentsList,
  type ComputeNodesList,
  type GrantLimits,
} from './compute-api'
import { getAccountAccessToken } from './K2ConnectSection'
import { listComputeMachines, ONE_CLICK_NEEDS_UPDATE, requestEnrollOffer, type ComputeMachine } from '../lib/compute-account'
import { controllerSub, mergeMachines, nodeNameFor, sasAgrees, type MachineRow } from '../lib/compute-machines'

/** Re-read the account's machines this often while the page is open. */
const ACCOUNT_POLL_MS = 30_000

type AccountState =
  | { kind: 'loading' }
  | { kind: 'signed_out' }
  | { kind: 'unavailable'; message: string }
  | { kind: 'error'; message: string }
  | { kind: 'ready'; machines: ComputeMachine[] }

function cores(n: number | null): string {
  if (n === null || !Number.isFinite(n) || n <= 0) return '—'
  return `${Number.isInteger(n) ? n : Math.round(n * 10) / 10} cores`
}

function displaySas(s: string | null | undefined): string {
  const t = (s ?? '').replace(/\s+/g, '')
  return /^\d{6}$/.test(t) ? `${t.slice(0, 3)} ${t.slice(3)}` : (s ?? '')
}

// ── 1. Machines you can use ──────────────────────────────────────────

export function MachinesPart({
  list,
  onChanged,
}: {
  list: ComputeNodesList | null
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const [account, setAccount] = useState<AccountState>({ kind: 'loading' })
  const [requested, setRequested] = useState<Record<string, number>>({})
  const [busyKey, setBusyKey] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [offerUnavailable, setOfferUnavailable] = useState(false)
  const mounted = useRef(true)
  const tokenRef = useRef<{ token: string; at: number } | null>(null)

  const token = useCallback(async (): Promise<string | null> => {
    const cached = tokenRef.current
    if (cached && Date.now() - cached.at < 10 * 60 * 1000) return cached.token
    const t = await getAccountAccessToken()
    tokenRef.current = t ? { token: t, at: Date.now() } : null
    return t
  }, [])

  const loadAccount = useCallback(async (): Promise<void> => {
    if (isAirgap()) {
      if (mounted.current) setAccount({ kind: 'unavailable', message: 'Air-gapped: add machines with an enroll code.' })
      return
    }
    const t = await token()
    if (!mounted.current) return
    if (!t) {
      setAccount({ kind: 'signed_out' })
      return
    }
    const res = await listComputeMachines(t)
    if (!mounted.current) return
    if (res.ok) setAccount({ kind: 'ready', machines: res.machines })
    else if (res.unavailable) setAccount({ kind: 'unavailable', message: res.message })
    else setAccount({ kind: 'error', message: res.message })
  }, [token])

  useEffect(() => {
    mounted.current = true
    void loadAccount()
    const id = setInterval(() => {
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return
      void loadAccount()
    }, ACCOUNT_POLL_MS)
    return () => {
      mounted.current = false
      clearInterval(id)
    }
  }, [loadAccount])

  const machines = account.kind === 'ready' ? account.machines : []
  const host = (list?.controller ?? '').toLowerCase()
  const sub = controllerSub(host)
  const rows = useMemo(
    () => mergeMachines(list?.nodes ?? [], machines, host, requested),
    [list?.nodes, machines, host, requested],
  )
  // One-click needs: the account table, a delivering ticket route, this
  // server on a k2.dev name, and a ticket key this server trusts.
  const oneClickMissing =
    account.kind === 'unavailable' || offerUnavailable || (account.kind === 'ready' && (!sub || list?.ticketSupported === false))

  const act = async (key: string, fn: () => Promise<void>): Promise<void> => {
    setBusyKey(key)
    setError(null)
    setNotice(null)
    try {
      await fn()
    } catch (err) {
      setError(computeErrorMessage(err))
    } finally {
      setBusyKey(null)
      void onChanged()
    }
  }

  const switchOn = async (row: MachineRow): Promise<void> => {
    if (row.status === 'retiring' && row.node) {
      const name = row.node.name
      await act(row.key, async () => {
        await pauseComputeNode(name, 'resume')
        setNotice(`${name} stays: it takes jobs again.`)
      })
      return
    }
    const m = row.machine
    if (!m || !sub) return
    await act(row.key, async () => {
      const t = await token()
      if (!t) {
        setAccount({ kind: 'signed_out' })
        return
      }
      const out = await requestEnrollOffer(t, { controllerSub: sub, nodeSub: m.sub, nodeFp: m.node_fp, nodeName: nodeNameFor(m) })
      if (!out.ok) {
        if (out.unavailable) setOfferUnavailable(true)
        else setError(out.message)
        return
      }
      setRequested((r) => ({ ...r, [m.sub]: Date.now() }))
      setNotice(`Asked ${row.name} to join; it checks in within a minute. Then confirm its code here.`)
    })
  }

  const switchOff = async (row: MachineRow): Promise<void> => {
    const n = row.node
    if (!n) return
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Stop using ${n.name}?`,
      message:
        n.running > 0
          ? `Its ${n.running} running job${n.running === 1 ? '' : 's'} finish first; queued jobs are cancelled. Then it's removed from this server. Switch it on again any time.`
          : "It's removed from this server; queued jobs are cancelled. Switch it on again any time.",
      confirmLabel: 'Stop using it',
      destructive: true,
    })
    if (!ok) return
    await act(row.key, async () => {
      const out = await retireComputeNode(n.name)
      setNotice(out.state === 'draining' ? `${n.name} finishes its jobs, then it's removed.` : `${n.name} removed.`)
    })
  }

  return (
    <SettingsGroup title="Machines you can use">
      <div className="space-y-2 py-1" data-settings-id="compute.machines" data-testid="compute-machines">
        <p className="text-[11px] text-[var(--color-text-muted)]">
          Your machines with “Make this computer a node” on. Switch one on and agents you allow below can run jobs there.
        </p>
        {account.kind === 'signed_out' ? (
          <Callout tone="info">
            Sign in to your k2.dev account (Settings → Tunnel) to list your machines. Machines added with a code still show here.
          </Callout>
        ) : null}
        {oneClickMissing ? (
          <Callout tone="info" data-testid="compute-oneclick-missing">
            {account.kind === 'unavailable' && account.message !== ONE_CLICK_NEEDS_UPDATE ? account.message : ONE_CLICK_NEEDS_UPDATE}
          </Callout>
        ) : null}
        {account.kind === 'error' ? <Callout tone="warn">{account.message}</Callout> : null}
        {account.kind === 'loading' && rows.length === 0 ? (
          <p className="text-[11px] text-[var(--color-text-muted)]">Loading…</p>
        ) : null}
        {account.kind !== 'loading' && rows.length === 0 ? (
          <p className="text-[11px] text-[var(--color-text-muted)]">
            No machines yet. On another computer, open Settings → Compute and turn on “Make this computer a node”, or add
            one with an enroll code below.
          </p>
        ) : null}
        {rows.map((row) => (
          <MachineLine
            key={row.key}
            row={row}
            busy={busyKey === row.key}
            canOneClick={!oneClickMissing && !!sub && account.kind === 'ready'}
            onOn={() => void switchOn(row)}
            onOff={() => void switchOff(row)}
            onConfirm={(sas) => {
              const n = row.node
              if (n) void act(row.key, async () => void (await confirmComputeNode(n.name, sas)))
            }}
          />
        ))}
        {notice ? <p className="text-[11px] text-[var(--color-text-secondary)]">{notice}</p> : null}
        {error ? <Callout tone="error">{error}</Callout> : null}
      </div>
    </SettingsGroup>
  )
}

const STATUS_TEXT: Record<MachineRow['status'], string> = {
  on: 'on',
  pending: 'waiting for your OK',
  retiring: 'finishing jobs, then removed',
  requested: 'asked to join…',
  off: 'off',
  elsewhere: 'used by another server',
}

function MachineLine({
  row,
  busy,
  canOneClick,
  onOn,
  onOff,
  onConfirm,
}: {
  row: MachineRow
  busy: boolean
  canOneClick: boolean
  onOn: () => void
  onOff: () => void
  onConfirm: (sas: string) => void
}): React.JSX.Element {
  const checked = row.status === 'on' || row.status === 'pending' || row.status === 'requested'
  const enrolledHere = !!row.node
  const disabled =
    busy || row.status === 'elsewhere' || row.status === 'requested' || (!enrolledHere && (!canOneClick || !row.online))
  const toggle = (next: boolean): void => {
    if (next) onOn()
    else if (enrolledHere) onOff()
  }
  const agree = sasAgrees(row.node, row.machine)
  return (
    <div className="border border-[var(--color-border)] p-2 space-y-1" data-testid={`compute-machine-${row.name}`}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Toggle
          checked={row.status === 'retiring' ? false : checked}
          onChange={toggle}
          disabled={disabled}
          aria-label={`Use compute from ${row.name}`}
        />
        <span className="text-xs font-medium text-[var(--color-text-primary)]">{row.name}</span>
        {row.sub && row.sub !== row.name ? (
          <span className="text-[11px] text-[var(--color-text-muted)]">{row.sub}.k2.dev</span>
        ) : null}
        <span className="text-[11px] text-[var(--color-text-muted)]">{[row.os, row.arch].filter(Boolean).join(' / ') || '—'}</span>
        <span className="text-[11px] text-[var(--color-text-secondary)]" data-testid="compute-machine-hw">
          {cores(row.cpuCores)} · {formatBytes(row.memBytes)} RAM · {formatBytes(row.diskFreeBytes)} free
        </span>
        <span className="text-[11px]" style={{ color: row.online ? 'var(--color-status-ok)' : 'var(--color-text-muted)' }}>
          {row.online ? 'online' : 'offline'}
        </span>
        <span className="text-[11px] text-[var(--color-text-secondary)]" data-testid="compute-machine-status">
          {row.status === 'elsewhere' && row.elsewhere ? `used by ${row.elsewhere}` : STATUS_TEXT[row.status]}
        </span>
      </div>
      {row.status === 'pending' && row.node ? (
        <div className="flex flex-wrap items-center gap-2 text-[11px] text-[var(--color-text-secondary)]">
          <span>
            This server&apos;s code: <span className="font-mono text-[var(--color-text-primary)]">{displaySas(row.node.sas)}</span>
            {row.machine?.sas ? (
              <>
                {' '}· {row.name} shows <span className="font-mono text-[var(--color-text-primary)]">{displaySas(row.machine.sas)}</span>
              </>
            ) : null}
          </span>
          {agree && row.machine?.sas ? (
            <Button variant="accent" disabled={busy} onClick={() => onConfirm((row.machine?.sas ?? '').replace(/\s+/g, ''))}>
              Codes match: confirm
            </Button>
          ) : row.machine?.sas ? (
            <span className="text-[var(--color-status-warn-amber-soft)]">The codes differ: don&apos;t confirm. Switch it off and on again.</span>
          ) : (
            <span className="text-[var(--color-text-muted)]">Type the code the machine shows in Node details below.</span>
          )}
        </div>
      ) : null}
    </div>
  )
}

// ── 2. Agents that can use compute ───────────────────────────────────

export function AgentsPart({
  agents,
  error,
  onChanged,
}: {
  agents: ComputeAgentsList | null
  error: string | null
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const [override, setOverride] = useState<Record<string, boolean>>({})
  // A write landed: the next read carries it, so drop the local view.
  useEffect(() => setOverride({}), [agents])
  const rows = agents?.agents ?? []
  return (
    <SettingsGroup title="Agents that can use compute">
      <div className="space-y-1 py-1" data-settings-id="compute.agents" data-testid="compute-agents">
        <p className="text-[11px] text-[var(--color-text-muted)]">
          On: the agent can run jobs on every machine switched on above (and ones you add later) with <span className="font-mono">k2 compute run</span>.
          Defaults: {Math.round((agents?.defaults.maxJobSecs ?? 7200) / 3600)} h per job, {agents?.defaults.maxParallel ?? 1} at a time,{' '}
          {agents?.defaults.maxQueued ?? 10} queued, disk up to a quarter of the machine&apos;s free space (at most{' '}
          {agents?.defaults.maxDiskGb ?? 60} GB).
        </p>
        {error ? <Callout tone="warn">{error}</Callout> : null}
        {agents && rows.length === 0 ? <p className="text-[11px] text-[var(--color-text-muted)]">No workspaces on this server.</p> : null}
        {!agents && !error ? <p className="text-[11px] text-[var(--color-text-muted)]">Loading…</p> : null}
        {rows.map((a) => (
          <AgentLine
            key={a.workspaceId}
            row={a}
            on={override[a.workspaceId] ?? a.agentsCanUseCompute}
            setOn={(v) => setOverride((o) => ({ ...o, [a.workspaceId]: v }))}
            onChanged={onChanged}
          />
        ))}
      </div>
    </SettingsGroup>
  )
}

const LIMIT_FIELDS: [keyof GrantLimits, string, string][] = [
  ['maxJobSecs', 'hours per job', 'h'],
  ['maxParallel', 'at a time', ''],
  ['maxQueued', 'queued', ''],
  ['maxDiskGb', 'disk GB', 'GB'],
]

function AgentLine({
  row,
  on,
  setOn,
  onChanged,
}: {
  row: ComputeAgentRow
  on: boolean
  setOn: (v: boolean) => void
  onChanged: () => Promise<void>
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [limitsOpen, setLimitsOpen] = useState(false)
  const first = row.grants[0]
  const [limits, setLimits] = useState<Record<keyof GrantLimits, string>>({
    maxJobSecs: '',
    maxParallel: '',
    maxQueued: '',
    maxDiskGb: '',
  })

  const send = async (value: boolean, body: GrantLimits = {}): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const out = await setComputeAgent(row.workspaceId, value, body)
      const now = typeof out.agentsCanUseCompute === 'boolean' ? out.agentsCanUseCompute : value
      setOn(now)
      // Optimistic projects-store patch (never fetchProjects() from here).
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) => (p.id === row.workspaceId ? { ...p, agentsCanUseCompute: now ? 1 : 0 } : p)),
      }))
    } catch (err) {
      setError(computeErrorMessage(err))
    } finally {
      setBusy(false)
      void onChanged()
    }
  }

  const saveLimits = async (): Promise<void> => {
    const body: GrantLimits = {}
    for (const [k] of LIMIT_FIELDS) {
      const raw = limits[k].trim()
      if (!raw) continue
      const n = Number(raw)
      if (!Number.isFinite(n) || n <= 0) {
        setError('Limits are positive numbers')
        return
      }
      body[k] = k === 'maxJobSecs' ? Math.round(n * 3600) : Math.round(n)
    }
    await send(true, body)
    setLimitsOpen(false)
  }

  const machines = row.grants.map((g) => g.node).filter((n): n is string => !!n)
  return (
    <div className="border-b border-[var(--color-border)] py-1 space-y-1" data-testid={`compute-agent-${row.workspace}`}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px]">
        <Toggle checked={on} onChange={(v) => void send(v)} disabled={busy} aria-label={`Use compute: ${row.name || row.workspace}`} />
        <span className="text-xs font-medium text-[var(--color-text-primary)]">{row.name || row.workspace}</span>
        <span className="text-[var(--color-text-muted)]" data-testid="compute-agent-machines">
          {on
            ? machines.length === 0
              ? 'no machines yet'
              : `${machines.length} machine${machines.length === 1 ? '' : 's'}: ${machines.join(', ')}`
            : 'off'}
        </span>
        {on && first ? (
          <span className="text-[var(--color-text-muted)]">
            ≤{Math.round(first.maxJobSecs / 360) / 10} h · {first.maxParallel} at a time · {first.maxQueued} queued · {first.maxDiskGb} GB
          </span>
        ) : null}
        {on ? (
          <Button variant="ghost" onClick={() => setLimitsOpen((o) => !o)} aria-expanded={limitsOpen}>
            limits…
          </Button>
        ) : null}
      </div>
      {on && limitsOpen ? (
        <div className="flex flex-wrap items-center gap-2 pl-10" data-testid="compute-agent-limits">
          {LIMIT_FIELDS.map(([k, label]) => (
            <Input
              key={k}
              value={limits[k]}
              onChange={(e) => setLimits((l) => ({ ...l, [k]: e.target.value.replace(/[^0-9.]/g, '') }))}
              placeholder={label}
              aria-label={`${label} for ${row.name || row.workspace}`}
              className="w-24"
            />
          ))}
          <Button variant="accent" disabled={busy} onClick={() => void saveLimits()}>
            Save limits
          </Button>
          <span className="text-[10px] text-[var(--color-text-muted)]">Blank = default. Applies to every machine.</span>
        </div>
      ) : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}
