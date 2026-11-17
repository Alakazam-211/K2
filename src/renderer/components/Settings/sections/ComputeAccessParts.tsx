// Settings → Compute, the everyday section (Rosson 2026-10-11):
//
//   Machines you can use — this server's nodes plus the account's
//   machines with "Make this computer a node" on. Each row's toggle
//   "Use compute from this machine": on = one-click enroll as a node of
//   THIS server (K2 Connect delivers the ticket to that machine); off =
//   drain, then remove (daemon `nodes/retire`).
//
// Which agents may use the machines is NOT set here: it's the Permissions
// tab's "Let this agent use compute nodes" (Settings → Workspaces / Agents →
// <agent> → Permissions; `ProjectsSection.tsx` AgentsUseComputeToggle).
//
// Thin client: the daemon owns nodes, grants and the retire sweep; K2
// Connect owns the account's machine list. The page only re-reads while
// open and sends the owner's gestures.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { SettingsGroup, SETTING_LIST_ROW_HOVER, SETTING_ROW_HINT, SETTING_ROW_LABEL, SETTING_SERVER_REASON } from '../controls/SettingControls'
import { Button, Input, Spinner, Toggle } from '@/components/ui'
import { useSettingsStore } from '@/stores/settings'
import { StatusDot, type HealthTone } from './ConnectionsParts'
import { ActionError, SettingsLink, failureFor, hardwareLine, osLine, type ActionFailure } from './ComputeBits'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { isAirgap } from '@/lib/airgap'
import {
  confirmComputeNode,
  pauseComputeNode,
  retireComputeNode,
  type ComputeNodesList,
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
  onManage,
}: {
  list: ComputeNodesList | null
  onChanged: () => Promise<void>
  /** Opens Machine settings (pause, resume, remove live there). */
  onManage?: () => void
}): React.JSX.Element {
  const [account, setAccount] = useState<AccountState>({ kind: 'loading' })
  const [requested, setRequested] = useState<Record<string, number>>({})
  const [busyKey, setBusyKey] = useState<string | null>(null)
  const [error, setError] = useState<ActionFailure | null>(null)
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

  const act = async (key: string, what: string, verb: string, fn: () => Promise<void>): Promise<void> => {
    setBusyKey(key)
    setError(null)
    setNotice(null)
    try {
      await fn()
    } catch (err) {
      setError(failureFor(what, verb, err))
    } finally {
      setBusyKey(null)
      void onChanged()
    }
  }

  const switchOn = async (row: MachineRow): Promise<void> => {
    if (row.status === 'retiring' && row.node) {
      const name = row.node.name
      await act(row.key, `Couldn’t keep ${name}.`, 'resume a machine', async () => {
        await pauseComputeNode(name, 'resume')
        setNotice(`${name} stays: it takes jobs again.`)
      })
      return
    }
    const m = row.machine
    if (!m || !sub) return
    await act(row.key, `Couldn’t ask ${row.name} to join.`, 'add a machine', async () => {
      const t = await token()
      if (!t) {
        setAccount({ kind: 'signed_out' })
        return
      }
      const out = await requestEnrollOffer(t, { controllerSub: sub, nodeSub: m.sub, nodeFp: m.node_fp, nodeName: nodeNameFor(m) })
      if (!out.ok) {
        if (out.unavailable) setOfferUnavailable(true)
        else setError({ what: `Couldn’t ask ${row.name} to join.`, next: 'Try again, or pair it with a pairing code below.', reason: out.message })
        return
      }
      setRequested((r) => ({ ...r, [m.sub]: Date.now() }))
      setNotice(`Asked ${row.name} to join. It checks in within a minute; then confirm its code here.`)
    })
  }

  const switchOff = async (row: MachineRow): Promise<void> => {
    const n = row.node
    if (!n) return
    const again = row.machine ? 'To use it again, switch it back on.' : 'To use it again, pair it again.'
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Remove ${n.name} from this server?`,
      message:
        n.running > 0
          ? `Its ${n.running} running ${n.running === 1 ? 'job finishes' : 'jobs finish'} first and queued jobs are cancelled; then it’s removed. ${again}`
          : `Its queued jobs are cancelled and it’s removed. ${again}`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (!ok) return
    await act(row.key, `Couldn’t remove ${n.name}.`, 'remove a machine', async () => {
      const out = await retireComputeNode(n.name)
      setNotice(out.state === 'draining' ? `${n.name} finishes its running jobs, then it’s removed. ${again}` : `${n.name} is removed. ${again}`)
    })
  }

  const reconnect = account.kind === 'signed_out' || account.kind === 'error'
  return (
    <SettingsGroup
      card
      title="Machines"
      hint={
        <span data-testid="compute-agents-note">
          Your machines that can run jobs for this server. Switch one on to use it. Which agents may use them is set on each
          agent’s page. <SettingsLink to="projects">Open Workspaces / Agents</SettingsLink>
        </span>
      }
      settingId="compute.machines"
      testId="compute-machines"
    >
      {account.kind === 'loading' && rows.length === 0 ? (
        <div className="px-4 py-3 flex items-center gap-2" role="status" aria-busy="true">
          <Spinner />
          <span className="type-hint text-[var(--color-text-secondary)]">Looking for your machines…</span>
        </div>
      ) : null}
      {account.kind !== 'loading' && rows.length === 0 ? (
        <div className="px-4 py-3" data-testid="compute-machines-empty">
          <p className={SETTING_ROW_LABEL}>No machines yet</p>
          <p className={SETTING_ROW_HINT}>
            Pair one with a pairing code below, or on the other computer open Compute and turn on Let a server run jobs on this
            computer.
          </p>
        </div>
      ) : null}
      {rows.map((row) => (
        <MachineLine
          key={row.key}
          row={row}
          busy={busyKey === row.key}
          canOneClick={!oneClickMissing && !!sub && account.kind === 'ready'}
          onOn={() => void switchOn(row)}
          onOff={() => void switchOff(row)}
          onManage={onManage}
          onConfirm={(sas) => {
            const n = row.node
            if (n) void act(row.key, `Couldn’t confirm ${n.name}.`, 'confirm a machine', async () => void (await confirmComputeNode(n.name, sas)))
          }}
        />
      ))}
      {reconnect || oneClickMissing ? (
        <div className="px-4 py-3 flex items-start justify-between gap-6" data-testid="compute-account-note">
          <div className="min-w-0">
            {account.kind === 'signed_out' ? (
              <>
                <p className={SETTING_ROW_LABEL}>Sign in to your k2.dev account to see all your machines</p>
                <p className={SETTING_ROW_HINT}>Machines paired with a pairing code show here either way.</p>
              </>
            ) : account.kind === 'error' ? (
              <>
                <p className={SETTING_ROW_LABEL}>Couldn’t list the machines on your k2.dev account</p>
                <p className={SETTING_ROW_HINT}>Machines paired with a pairing code still show. K2 tries again every 30 seconds.</p>
                <p className={SETTING_SERVER_REASON}>The server said: {account.message.replace(/[.\s]+$/, '')}.</p>
              </>
            ) : (
              <>
                <p className={SETTING_ROW_LABEL}>One-click pairing isn’t available here yet</p>
                <p className={SETTING_ROW_HINT} data-testid="compute-oneclick-missing">
                  {account.kind === 'unavailable' && account.message !== ONE_CLICK_NEEDS_UPDATE
                    ? account.message
                    : 'Your k2.dev account needs an update for it. Until then, pair a machine with a pairing code below.'}
                </p>
              </>
            )}
          </div>
          {account.kind === 'signed_out' ? (
            <Button onClick={() => useSettingsStore.getState().openSettings('k2-connect')}>Sign in…</Button>
          ) : account.kind === 'error' ? (
            <Button onClick={() => void loadAccount()}>Try again</Button>
          ) : null}
        </div>
      ) : null}
      {notice || error ? (
        <div className="px-4 py-3">
          {notice ? (
            <p role="status" className="type-hint text-[var(--color-text-secondary)]">
              {notice}
            </p>
          ) : null}
          <ActionError failure={error} />
        </div>
      ) : null}
    </SettingsGroup>
  )
}

const STATUS_TEXT: Record<MachineRow['status'], string> = {
  on: 'In use',
  pending: 'Waiting for your OK',
  retiring: 'Finishing its jobs, then removed',
  requested: 'Asked to join…',
  off: 'Not in use',
  elsewhere: 'Used by another server',
}

const STATUS_TONE: Record<MachineRow['status'], HealthTone> = {
  on: 'ok',
  pending: 'warn',
  retiring: 'warn',
  requested: 'neutral',
  off: 'neutral',
  elsewhere: 'neutral',
}

function MachineLine({
  row,
  busy,
  canOneClick,
  onOn,
  onOff,
  onConfirm,
  onManage,
}: {
  row: MachineRow
  busy: boolean
  canOneClick: boolean
  onOn: () => void
  onOff: () => void
  onConfirm: (sas: string) => void
  onManage?: () => void
}): React.JSX.Element {
  const [typed, setTyped] = useState('')
  const checked = row.status === 'on' || row.status === 'pending' || row.status === 'requested'
  const enrolledHere = !!row.node
  const disabled =
    busy || row.status === 'elsewhere' || row.status === 'requested' || (!enrolledHere && (!canOneClick || !row.online))
  const toggle = (next: boolean): void => {
    if (next) onOn()
    else if (enrolledHere) onOff()
  }
  const agree = sasAgrees(row.node, row.machine)
  // A paused node is still in use; say so rather than "In use".
  const pause = row.node?.controllerPause
  const statusText =
    row.status === 'elsewhere' && row.elsewhere
      ? `Used by ${row.elsewhere}`
      : row.status === 'on' && pause
        ? pause === 'paused'
          ? 'Paused'
          : 'Finishing its jobs'
        : STATUS_TEXT[row.status]
  const tone: HealthTone = row.status === 'on' && pause ? 'warn' : STATUS_TONE[row.status]
  const desc = [osLine(row.os, row.arch), row.sub && row.sub !== row.name ? `${row.sub}.k2.dev` : null].filter(Boolean).join(' · ')
  return (
    <div className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`compute-machine-${row.name}`}>
      <div className="flex items-start justify-between gap-6">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <span className="type-name text-[var(--color-text-primary)]">{row.name}</span>
            <span className="inline-flex items-center gap-1.5 type-hint text-[var(--color-text-secondary)]">
              <StatusDot tone={row.online ? tone : 'neutral'} />
              <span data-testid="compute-machine-status">{statusText}</span>
              <span>· {row.online ? 'online' : 'offline'}</span>
            </span>
          </div>
          <div className={SETTING_ROW_HINT}>
            {desc}
            {row.cpuCores === null && row.memBytes === null ? (
              row.status === 'pending' ? ' · reports its hardware once confirmed' : ''
            ) : (
              <>
                {desc ? ' · ' : ''}
                <span data-testid="compute-machine-hw">{hardwareLine(cores(row.cpuCores), row.memBytes, row.diskFreeBytes)}</span>
              </>
            )}
          </div>
          {row.status === 'on' && pause ? (
            <p className={SETTING_ROW_HINT} data-testid="compute-machine-paused">
              Still paired, but paused: it gets no new jobs until you resume it.{' '}
              {onManage ? (
                <button type="button" data-k2-focus="" onClick={onManage} className="relative px-0.5 -mx-0.5 type-hint text-[var(--color-accent-text)] underline underline-offset-2 no-drag cursor-pointer">
                  Resume in Machine settings
                </button>
              ) : null}
            </p>
          ) : null}
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          <Toggle
            checked={row.status === 'retiring' ? false : checked}
            onChange={toggle}
            disabled={disabled}
            aria-label={`Use compute from ${row.name}`}
          />
        </div>
      </div>
      {row.status === 'pending' && row.node ? (
        <div className="mt-2 flex flex-wrap items-center gap-2 type-hint text-[var(--color-text-secondary)]" data-testid={`compute-machine-confirm-${row.name}`}>
          {row.machine?.sas ? (
            <>
              <span>
                This server’s code <span className="tabular-nums text-[var(--color-text-primary)]">{displaySas(row.node.sas)}</span> · {row.name}{' '}
                shows <span className="tabular-nums text-[var(--color-text-primary)]">{displaySas(row.machine.sas)}</span>
              </span>
              {agree ? (
                <Button variant="accent" disabled={busy} onClick={() => onConfirm((row.machine?.sas ?? '').replace(/\s+/g, ''))}>
                  Codes match: confirm
                </Button>
              ) : (
                <span className="text-[var(--color-status-warn-text)]">The codes differ: don’t confirm. Switch it off and on again.</span>
              )}
            </>
          ) : row.node.sas ? (
            <>
              <span className="type-row text-[var(--color-text-primary)]">
                Does {row.name} show <span className="tabular-nums">{displaySas(row.node.sas)}</span>?
              </span>
              <Button variant="accent" disabled={busy} onClick={() => onConfirm((row.node?.sas ?? '').replace(/\s+/g, ''))}>
                Yes, that’s it
              </Button>
              <span className="basis-full">If the machine shows a different code, don’t confirm: switch it off to remove it.</span>
            </>
          ) : (
            <>
              <span>Type the 6-digit code {row.name} shows.</span>
              <Input value={typed} onChange={(e) => setTyped(e.target.value)} placeholder="123 456" aria-label={`Code ${row.name} shows`} className="w-28" />
              <Button variant="accent" disabled={busy || typed.trim().length === 0} onClick={() => onConfirm(typed.replace(/\s+/g, ''))}>
                Confirm
              </Button>
            </>
          )}
        </div>
      ) : null}
    </div>
  )
}
