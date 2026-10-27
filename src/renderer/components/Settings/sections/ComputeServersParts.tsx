// Settings → Compute, node sharing (prd-compute-shared-queue-v1 §9;
// prd-macos-tenant-compute-v1 §5, MT21–MT26).
//
// Daemon-first: every fact here is what `compute/nodes/servers`,
// `compute/queue` and the nodes JSON report, and every button is one
// gesture posted back. The daemon decides whether a limits change narrows
// (at once) or widens (held for approval), who may revoke or change the
// owner, and which server may take over; this page never computes sharing
// state. No polling loop of its own: the lists load when opened, after each
// action, and on Refresh; the node card above re-renders with the page's
// own 5 s poll.
//
//   - NodeSharingPanel: per node. On a node this server owns (the offer has
//     `servers`): the Servers panel and the Queue. On a node another server
//     owns: "Shared by …", this server's share and the Queue; no admin.

import React, { useCallback, useEffect, useState } from 'react'
import { Button, Callout, Input, SquareCheckbox, SquareRadio, Toggle } from '@/components/ui'
import {
  addNodeServer,
  changeNodeOwner,
  computeErrorMessage,
  fetchNodeQueue,
  fetchNodeServers,
  formatCores,
  limitsLine,
  ownerTargets,
  pauseServer,
  revokeServer,
  setServerLimits,
  setServerPriority,
  teamPossible,
  tierTag,
  type ComputeNode,
  type NodeQueue,
  type NodeServerRow,
  type NodeServers,
  type NodeServersOwner,
  type ServersChangeResult,
  type ShareLimitsJson,
} from './compute-api'

const muted = 'text-[var(--color-text-muted)]'
const secondary = 'text-[var(--color-text-secondary)]'
const primary = 'text-[var(--color-text-primary)]'

function nowSec(): number {
  return Math.floor(Date.now() / 1000)
}

function endDate(at: number | null | undefined): string {
  if (!at) return 'no end date'
  const left = at - nowSec()
  if (left <= 0) return 'ended'
  return `ends ${new Date(at * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}`
}

function cpuHours(secs: number | null | undefined): string {
  return `${((secs ?? 0) / 3600).toFixed(1)} CPU-h today`
}

/** "7d", "12h", "none" → seconds (0 = no end date); null = not valid. */
export function parseForSecs(v: string): number | null {
  const t = v.trim().toLowerCase()
  if (t === '') return null
  if (t === 'none' || t === 'never' || t === '0') return 0
  const m = /^(\d+)\s*([dhm])$/.exec(t)
  if (!m) return null
  const n = Number(m[1])
  const mul = m[2] === 'd' ? 86400 : m[2] === 'h' ? 3600 : 60
  return n * mul
}

/** What a change answered, in words: held for approval, or done. */
function changeWords(r: ServersChangeResult | undefined): string | null {
  if (!r) return null
  if (r.held) return 'Waiting for your approval (Approvals above, or the card).'
  return r.note ?? null
}

// ── per node ─────────────────────────────────────────────────────────

export function NodeSharingPanel({ node }: { node: ComputeNode }): React.JSX.Element | null {
  const features = Array.isArray(node.offer?.features) ? node.offer.features : []
  if (node.role === 'tenant') {
    return (
      <div className="space-y-2" data-testid={`compute-sharing-${node.name}`}>
        <TenantShare node={node} />
        <QueuePart node={node.name} />
      </div>
    )
  }
  if (!features.includes('servers')) return null
  return (
    <div className="space-y-2" data-testid={`compute-sharing-${node.name}`}>
      <ServersPanel node={node} />
      {node.share?.shared ? <QueuePart node={node.name} /> : null}
    </div>
  )
}

// ── a node another server owns ───────────────────────────────────────

function TenantShare({ node }: { node: ComputeNode }): React.JSX.Element {
  const s = node.share
  return (
    <div className="space-y-1" data-testid="compute-tenant-share">
      <div className={`text-[11px] ${primary}`}>
        Shared by <span className="font-medium">{s?.ownerLabel ?? 'another server'}</span>
        {s?.servers ? ` (${s.servers} servers)` : ''}. Its admin, updates and servers are that server&apos;s.
      </div>
      <div className={`text-[11px] ${secondary}`}>
        Your share: {s?.state ?? '—'} · {s?.running ?? 0} running · {s?.queued ?? 0} queued · {cpuHours(s?.cpuSecsToday)} · {endDate(s?.expiresAt)}
      </div>
      {s?.limits ? <div className={`text-[10px] ${muted}`}>{limitsLine(s.limits)}</div> : null}
    </div>
  )
}

// ── the queue ────────────────────────────────────────────────────────

function QueuePart({ node }: { node: string }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [q, setQ] = useState<NodeQueue | null>(null)
  const [error, setError] = useState<string | null>(null)
  const load = useCallback(async () => {
    try {
      setQ(await fetchNodeQueue(node))
      setError(null)
    } catch (e) {
      setError(computeErrorMessage(e))
    }
  }, [node])
  useEffect(() => {
    if (open) void load()
  }, [open, load])
  return (
    <div className="space-y-1">
      <div className="flex items-center gap-2">
        <Button variant="quiet" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
          {open ? 'Hide queue' : 'Queue'}
        </Button>
        {open ? (
          <Button variant="ghost" onClick={() => void load()}>
            Refresh
          </Button>
        ) : null}
      </div>
      {open ? (
        <div className="space-y-0.5" data-testid={`compute-queue-${node}`}>
          {error ? <Callout tone="error">{error}</Callout> : null}
          {q && q.entries.length === 0 ? <p className={`text-[11px] ${muted}`}>Nothing running or queued.</p> : null}
          {(q?.entries ?? []).map((e) => (
            <div key={`${e.pos}-${e.short ?? ''}`} className={`text-[11px] flex flex-wrap gap-x-3 ${secondary}`}>
              <span className={muted}>#{e.pos}</span>
              <span className={primary}>{e.mine ? (e.short ?? 'yours') : e.server ? e.server : 'another server'}</span>
              <span>{e.state}</span>
              <span>{e.class}</span>
              {tierTag(e.tier) ? (
                <span className={`font-medium ${primary}`} data-testid="compute-queue-tier">
                  {tierTag(e.tier)}
                </span>
              ) : null}
              {e.cpuMillis ? <span>{formatCores(e.cpuMillis)}</span> : null}
              {typeof e.etaSecs === 'number' ? <span>~{Math.max(1, Math.round(e.etaSecs / 60))} min</span> : null}
              {e.blockedBy ? <span className={muted}>{e.blockedBy.replace(/_/g, ' ')}</span> : null}
              {e.mine && e.argv ? <span className="font-mono truncate max-w-full">{e.argv}</span> : null}
            </div>
          ))}
        </div>
      ) : null}
    </div>
  )
}

// ── the servers on a node this server owns ───────────────────────────

function ServersPanel({ node }: { node: ComputeNode }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [data, setData] = useState<NodeServers | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)

  const load = useCallback(
    async (refresh = false) => {
      try {
        setData(await fetchNodeServers(node.name, refresh))
        setError(null)
      } catch (e) {
        setError(computeErrorMessage(e))
      }
    },
    [node.name],
  )
  useEffect(() => {
    if (open) void load()
  }, [open, load])

  /** One gesture, then read the list again. */
  const act = useCallback(
    async (fn: () => Promise<ServersChangeResult>): Promise<boolean> => {
      setNotice(null)
      setError(null)
      try {
        const r = await fn()
        setNotice(changeWords(r))
        return true
      } catch (e) {
        setError(computeErrorMessage(e))
        return false
      } finally {
        void load()
      }
    },
    [load],
  )

  const owner = data && data.role === 'owner' ? (data as NodeServersOwner) : null
  const rows = owner?.servers ?? []
  const count = rows.filter((r) => r.state === 'active' || r.state === 'paused').length
  const team = teamPossible(node)
  const targets = ownerTargets(rows)
  const canMove = Array.isArray(node.offer?.features) && node.offer.features.includes('servers-owner')

  return (
    <div className="space-y-2" data-testid={`compute-servers-${node.name}`} data-settings-id="compute.servers">
      <div className="flex items-center gap-2">
        <Button variant="quiet" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
          {open ? 'Hide servers' : 'Servers'}
        </Button>
        {open ? (
          <>
            <Button variant="ghost" onClick={() => void load(true)}>
              Refresh
            </Button>
            <Button variant="ghost" onClick={() => setAdding((v) => !v)} aria-expanded={adding}>
              Add server
            </Button>
          </>
        ) : null}
      </div>
      {open ? (
        <div className="space-y-2">
          {error ? <Callout tone="error">{error}</Callout> : null}
          {notice ? (
            <Callout tone="info" data-testid="compute-servers-notice">
              {notice}
            </Callout>
          ) : null}
          {owner ? (
            <div className={`text-[11px] ${secondary}`} data-testid="compute-servers-header">
              {count} of {owner.maxServers ?? '?'} servers ·{' '}
              {team.ok ? 'takes other people’s servers' : `other people’s servers: no (${team.why})`}
            </div>
          ) : null}
          {owner?.note ? <p className={`text-[10px] ${muted}`}>{owner.note}</p> : null}
          {(owner?.requests ?? [])
            .filter((r) => r.state === 'pending')
            .map((r) => (
              <div key={r.id} className={`text-[11px] ${secondary}`} data-testid={`compute-servers-waiting-${r.id}`}>
                Waiting for your approval: {r.summary}
              </div>
            ))}
          {adding ? (
            <AddServerForm
              team={team}
              onDone={() => setAdding(false)}
              onSubmit={(body) => act(() => addNodeServer(node.name, body))}
            />
          ) : null}
          {rows.map((r) => (
            <ServerRow
              key={`${r.slot}-${r.label}`}
              node={node.name}
              row={r}
              ownerActs={owner?.ownerActs === true}
              canBeOwner={canMove && targets.some((t) => t.slot === r.slot)}
              act={act}
            />
          ))}
        </div>
      ) : null}
    </div>
  )
}

type Pane = 'none' | 'limits' | 'revoke' | 'owner'

function ServerRow({
  node,
  row: r,
  ownerActs,
  canBeOwner,
  act,
}: {
  node: string
  row: NodeServerRow
  ownerActs: boolean
  canBeOwner: boolean
  act: (fn: () => Promise<ServersChangeResult>) => Promise<boolean>
}): React.JSX.Element {
  const [pane, setPane] = useState<Pane>('none')
  const isOwner = r.slot === 0
  const server = r.label
  const toggle = (p: Pane): void => setPane((cur) => (cur === p ? 'none' : p))
  return (
    <div className="border-t border-[var(--color-border)] pt-1 space-y-1" data-testid={`compute-server-row-${r.slot}`}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-[11px]">
        <span className={`font-medium ${primary}`}>{r.label}</span>
        <span className={muted}>slot {r.slot}</span>
        {isOwner ? <span className={`font-medium ${secondary}`}>owner</span> : null}
        <span className={secondary} data-testid="compute-server-state">
          {r.state}
        </span>
        <span className={secondary}>{r.trust === 'team' ? 'other person’s' : 'yours'}</span>
        {r.usage ? (
          <span className={secondary}>
            {r.usage.running ?? 0} running · {r.usage.queued ?? 0} queued · {cpuHours(r.usage.cpu_secs_today)}
            {r.usage.online ? ' · online' : ' · offline'}
          </span>
        ) : null}
        {!isOwner ? <span className={muted}>{endDate(r.expiresAt)}</span> : null}
      </div>
      {r.limits ? <div className={`text-[10px] ${muted}`}>{limitsLine(r.limits)}</div> : null}
      {/* Job tiers (0.46.5): a guest server's Priority counts here only when
          the node's owner says so; turning it on is a widening (held for
          approval). The owner's own line always counts. Absent on older
          daemons: nothing shown. */}
      {!isOwner && r.priority !== undefined && r.state !== 'revoked' ? (
        <div className="flex items-center gap-2 text-[11px]" data-testid="compute-server-priority">
          <Toggle
            checked={r.priority === true}
            disabled={r.priority === null}
            onChange={(v) => void act(() => setServerPriority(node, server, v))}
            aria-label={`Priority counts here: ${r.label}`}
          />
          <span className={secondary}>Priority counts here</span>
          {r.priority === null ? <span className={muted}>needs k2-node 0.46.5</span> : null}
        </div>
      ) : null}
      {r.sas && r.state === 'pending' ? (
        <div className={`text-[11px] ${primary}`} data-testid="compute-server-sas">
          SAS <span className="font-mono">{r.sas}</span>: read it to {r.label}&apos;s owner.
        </div>
      ) : null}
      {r.refusal && r.state === 'refused' ? (
        <div className="text-[11px] text-[var(--color-status-warn-amber-soft)]">Its attach is refused: {r.refusal.replace(/_/g, ' ')}</div>
      ) : null}
      {r.solo ? null : (
        <div className="flex flex-wrap items-center gap-2">
          {r.state !== 'revoked' ? (
            <Button variant="quiet" onClick={() => toggle('limits')} aria-expanded={pane === 'limits'}>
              Edit limits
            </Button>
          ) : null}
          {!isOwner && r.state === 'active' ? (
            <Button onClick={() => void act(() => pauseServer(node, server, 'pause'))}>Pause</Button>
          ) : null}
          {!isOwner && r.state === 'paused' ? (
            <Button onClick={() => void act(() => pauseServer(node, server, 'resume'))}>Resume</Button>
          ) : null}
          {!isOwner ? (
            <Button
              variant="quiet"
              disabled={!ownerActs}
              title={ownerActs ? undefined : 'Only the server’s owner revokes a server'}
              onClick={() => toggle('revoke')}
              aria-expanded={pane === 'revoke'}
            >
              Revoke
            </Button>
          ) : null}
          {canBeOwner ? (
            <Button
              variant="quiet"
              disabled={!ownerActs}
              title={ownerActs ? undefined : 'Only the server’s owner changes the node’s owner'}
              onClick={() => toggle('owner')}
              aria-expanded={pane === 'owner'}
            >
              Make owner
            </Button>
          ) : null}
        </div>
      )}
      {pane === 'limits' && r.limits ? (
        <LimitsForm
          row={r}
          onCancel={() => setPane('none')}
          onSave={async (set, forSecs) => {
            const ok = await act(() => setServerLimits(node, server, set, forSecs))
            if (ok) setPane('none')
          }}
        />
      ) : null}
      {pane === 'revoke' ? (
        <RevokeConfirm
          label={r.label}
          onCancel={() => setPane('none')}
          onConfirm={async (now, wipe) => {
            const ok = await act(() => revokeServer(node, server, { now, wipe }))
            if (ok) setPane('none')
          }}
        />
      ) : null}
      {pane === 'owner' ? (
        <OwnerConfirm
          node={node}
          label={r.label}
          fp={r.fp ?? ""}
          onCancel={() => setPane('none')}
          onConfirm={async (opts) => {
            const ok = await act(() => changeNodeOwner(node, server, opts))
            if (ok) setPane('none')
          }}
        />
      ) : null}
    </div>
  )
}

const LIMIT_FIELDS: { key: keyof ShareLimitsJson; label: string }[] = [
  { key: 'max_running', label: 'Running at once' },
  { key: 'max_queued', label: 'Queued' },
  { key: 'cpu_hours_day', label: 'CPU-hours a day' },
  { key: 'max_cpus', label: 'CPUs per job' },
  { key: 'max_mem_gb', label: 'Memory per job (GB)' },
  { key: 'disk_gb', label: 'Disk (GB)' },
  { key: 'weight', label: 'Weight' },
]

/** The fields that changed, as the daemon takes them. The daemon decides
 *  narrow (at once) vs widen (held); this form only sends the numbers. */
export function changedLimits(base: ShareLimitsJson, edited: Record<string, string>): Partial<ShareLimitsJson> {
  const out: Partial<ShareLimitsJson> = {}
  for (const { key } of LIMIT_FIELDS) {
    const raw = (edited[key] ?? '').trim()
    if (raw === '') continue
    const n = Number(raw)
    if (Number.isInteger(n) && n !== base[key]) (out as Record<string, number>)[key] = n
  }
  return out
}

function LimitsForm({
  row,
  onSave,
  onCancel,
}: {
  row: NodeServerRow
  onSave: (set: Partial<ShareLimitsJson>, forSecs?: number) => Promise<void>
  onCancel: () => void
}): React.JSX.Element {
  const base = row.limits as ShareLimitsJson
  const [vals, setVals] = useState<Record<string, string>>(() => Object.fromEntries(LIMIT_FIELDS.map(({ key }) => [key, String(base[key])])))
  const [forv, setForv] = useState('')
  const [err, setErr] = useState<string | null>(null)
  const isOwner = row.slot === 0
  const save = async (): Promise<void> => {
    const set = changedLimits(base, vals)
    let forSecs: number | undefined
    if (!isOwner && forv.trim() !== '') {
      const f = parseForSecs(forv)
      if (f === null) {
        setErr('End date: e.g. 7d, 12h, or none')
        return
      }
      forSecs = f
    }
    if (Object.keys(set).length === 0 && forSecs === undefined) {
      setErr('Nothing changed')
      return
    }
    setErr(null)
    await onSave(set, forSecs)
  }
  return (
    <div className="space-y-1 border border-[var(--color-border)] p-2" data-testid="compute-limits-form">
      <p className={`text-[10px] ${muted}`}>A lower limit applies at once. A higher one waits for your approval.</p>
      <div className="grid grid-cols-2 gap-1">
        {LIMIT_FIELDS.map(({ key, label }) => (
          <label key={key} className={`text-[11px] ${secondary} flex items-center gap-2`}>
            <span className="w-36">{label}</span>
            <Input value={vals[key] ?? ''} onChange={(e) => setVals((v) => ({ ...v, [key]: e.target.value }))} className="w-20" />
          </label>
        ))}
        {!isOwner ? (
          <label className={`text-[11px] ${secondary} flex items-center gap-2`}>
            <span className="w-36">End date from now</span>
            <Input value={forv} onChange={(e) => setForv(e.target.value)} placeholder="7d, none" className="w-20" />
          </label>
        ) : null}
      </div>
      {err ? <Callout tone="warn">{err}</Callout> : null}
      <div className="flex gap-2">
        <Button variant="accent" onClick={() => void save()}>
          Save
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  )
}

function RevokeConfirm({
  label,
  onConfirm,
  onCancel,
}: {
  label: string
  onConfirm: (now: boolean, wipe: boolean) => Promise<void>
  onCancel: () => void
}): React.JSX.Element {
  const [now, setNow] = useState(false)
  const [wipe, setWipe] = useState(false)
  return (
    <div role="dialog" aria-label={`Revoke ${label}?`} className="space-y-1 border border-[var(--color-border)] p-2" data-testid="compute-revoke-confirm">
      <div className={`text-[11px] font-medium ${primary}`}>Revoke {label}?</div>
      <p className={`text-[11px] ${secondary}`}>Its queued jobs are cancelled and it takes no new work here. Its files are wiped in 24 hours.</p>
      <label className={`text-[11px] ${secondary} flex items-center gap-2`}>
        <SquareCheckbox checked={now} onChange={(e) => setNow(e.target.checked)} aria-label="Stop running jobs now" />
        Stop its running jobs now (else they finish)
      </label>
      <label className={`text-[11px] ${secondary} flex items-center gap-2`}>
        <SquareCheckbox checked={wipe} onChange={(e) => setWipe(e.target.checked)} aria-label="Wipe its files now" />
        Wipe its files now (else in 24 hours)
      </label>
      <div className="flex gap-2">
        <Button variant="danger" onClick={() => void onConfirm(now, wipe)}>
          Revoke
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  )
}

function OwnerConfirm({
  node,
  label,
  fp,
  onConfirm,
  onCancel,
}: {
  node: string
  label: string
  fp: string
  onConfirm: (opts: { old: 'keep' | 'revoke'; caches: 'move' | 'wipe'; now: boolean }) => Promise<void>
  onCancel: () => void
}): React.JSX.Element {
  const [leave, setLeave] = useState(false)
  const [wipe, setWipe] = useState(false)
  const [now, setNow] = useState(false)
  return (
    <div role="dialog" aria-label={`Make ${label} the owner of ${node}?`} className="space-y-1 border border-[var(--color-border)] p-2" data-testid="compute-owner-confirm">
      <div className={`text-[11px] font-medium ${primary}`}>
        Make {label} the owner of {node}?
      </div>
      <p className={`text-[11px] ${secondary}`}>
        {label} gets this node&apos;s admin, updates and approvals, and pins its own approval key. After you approve, the node drains (running jobs
        finish, up to 30 min), the two servers swap places, root re-pins to {label}, and k2-node restarts once. Queued jobs stay with their server;
        other servers on the node are untouched.
      </p>
      <p className={`text-[11px] ${secondary}`} data-testid="compute-owner-key">
        Its key: <span className={`font-mono break-all ${primary}`}>{fp}</span>. Check it matches what {label} shows (controllerFp in{' '}
        <span className="font-mono">k2 compute nodes --json</span> there) before you approve.
      </p>
      <label className={`text-[11px] ${secondary} flex items-center gap-2`}>
        <SquareCheckbox checked={leave} onChange={(e) => setLeave(e.target.checked)} aria-label="Remove this server from the node" />
        Remove this server from the node (else it stays as one of your servers)
      </label>
      <label className={`text-[11px] ${secondary} flex items-center gap-2`}>
        <SquareCheckbox checked={wipe} onChange={(e) => setWipe(e.target.checked)} aria-label="Wipe the build caches" />
        Wipe both servers&apos; build caches (else they move with their server)
      </label>
      <label className={`text-[11px] ${secondary} flex items-center gap-2`}>
        <SquareCheckbox checked={now} onChange={(e) => setNow(e.target.checked)} aria-label="Stop running jobs now" />
        Stop running jobs now (else they finish first)
      </label>
      <div className="flex gap-2">
        <Button variant="danger" onClick={() => void onConfirm({ old: leave ? 'revoke' : 'keep', caches: wipe ? 'wipe' : 'move', now })}>
          Ask to change the owner
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  )
}

function AddServerForm({
  team,
  onSubmit,
  onDone,
}: {
  team: { ok: boolean; why: string | null }
  onSubmit: (body: { url: string; code: string; trust: 'own' | 'team'; forSecs?: number }) => Promise<boolean>
  onDone: () => void
}): React.JSX.Element {
  const [url, setUrl] = useState('')
  const [code, setCode] = useState('')
  const [trust, setTrust] = useState<'own' | 'team'>('own')
  const [forv, setForv] = useState('')
  const [err, setErr] = useState<string | null>(null)
  const submit = async (): Promise<void> => {
    if (!url.trim() || !code.trim()) {
      setErr('The new server’s address and the CODE.FP its `k2 compute node add` printed')
      return
    }
    let forSecs: number | undefined
    if (forv.trim() !== '') {
      const f = parseForSecs(forv)
      if (f === null) {
        setErr('End date: e.g. 7d, 12h, or none')
        return
      }
      forSecs = f
    }
    setErr(null)
    if (await onSubmit({ url: url.trim(), code: code.trim(), trust, forSecs })) onDone()
  }
  return (
    <div className="space-y-1 border border-[var(--color-border)] p-2" data-testid="compute-add-server">
      <label className={`text-[11px] ${secondary} flex items-center gap-2`}>
        <span className="w-24">Address</span>
        <Input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://baden.k2.dev" aria-label="New server address" className="flex-1" />
      </label>
      <label className={`text-[11px] ${secondary} flex items-center gap-2`}>
        <span className="w-24">Code</span>
        <Input value={code} onChange={(e) => setCode(e.target.value)} placeholder="ABCDE-FGHJK.0123456789abcdef" aria-label="Enroll code" className="flex-1 font-mono" />
      </label>
      <div className={`text-[11px] ${secondary} flex items-center gap-3`}>
        <label className="flex items-center gap-1">
          <SquareRadio name="trust" checked={trust === 'own'} onChange={() => setTrust('own')} aria-label="One of your own servers" /> Yours
        </label>
        <label className="flex items-center gap-1" title={team.ok ? undefined : (team.why ?? undefined)}>
          <SquareRadio name="trust" checked={trust === 'team'} disabled={!team.ok} onChange={() => setTrust('team')} aria-label="Another person’s server" />{' '}
          Another person&apos;s
        </label>
        {!team.ok ? <span className={muted}>({team.why})</span> : null}
      </div>
      <label className={`text-[11px] ${secondary} flex items-center gap-2`}>
        <span className="w-24">End date</span>
        <Input value={forv} onChange={(e) => setForv(e.target.value)} placeholder={trust === 'team' ? '7d (default)' : 'none (default)'} className="w-28" />
      </label>
      {trust === 'team' ? (
        <p className={`text-[10px] ${muted}`}>
          Its jobs get their own user and folders here and can&apos;t read yours. They do share the machine&apos;s kernel, CPU and disk and can reach
          the internet and services on this machine&apos;s loopback. Only add people you trust.
        </p>
      ) : null}
      <p className={`text-[10px] ${muted}`}>The add waits for your approval; then read the SAS shown here to the new server&apos;s owner.</p>
      {err ? <Callout tone="warn">{err}</Callout> : null}
      <div className="flex gap-2">
        <Button variant="accent" onClick={() => void submit()}>
          Add
        </Button>
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
      </div>
    </div>
  )
}
