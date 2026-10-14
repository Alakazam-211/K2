// Review a proposed source, and approve it (contract-sources-v1 §4.3, §8;
// PRD §9.5). Owner: B2.
//
// What a person sees before saying yes: the site (origin and path, query
// values masked), how often, what it keeps, who proposed it, a Try with the
// shape tree, who may read it (`viewers`), what agents see (`agentAccess`),
// whether values are kept at all (`keep`), and the secret it uses.
//
// One click for a secretless source on a public host (Q2). With a secret:
// the vault confirm (`source-secret`, bound to the source id) [GV43], then
// approve. A root key gets a warning and "Use anyway" (Q5). The approval
// carries the `specHash` this person saw; a changed spec is refused (409
// `spec_changed`) and the review reloads.

import React, { useEffect, useMemo, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type {
  AgentAccess,
  Keep,
  PendingSource,
  RootKeyHint,
  SourceShowResponse,
  SourceTryResponse,
  Viewers,
} from '@/lib/sources/sources-types'
import { approveSource, showSource, trySource } from '@/lib/sources/sources-api'
import { vaultApi, type VaultSecret } from './vault-api'
import { useSettingsStore } from '@/stores/settings'
import { ConfirmStep } from './DataSourceConfirm'
import { DataSourceTryTree, TrySummary } from './DataSourceTryTree'
import { btn, btnPrimary, danger, every, makerLabel, maskQuery, muted, sourcesErrorOf, sourcesErrorText } from './data-sources-ui'
import { SquareRadio } from '@/components/ui'

/** Does a vault secret's site list include this origin? */
export function secretFitsOrigin(s: Pick<VaultSecret, 'hosts' | 'status'>, origin: string): boolean {
  if (s.status !== 'active') return false
  const host = (o: string): string => {
    try {
      return new URL(o).hostname
    } catch {
      return o
    }
  }
  return s.hosts.some((h) => h === origin || host(h) === host(origin))
}

function fetchLines(show: SourceShowResponse | null): string[] {
  const f = show?.source.fetch
  if (!f) return []
  if ('url' in f) return [maskQuery(f.url)]
  if ('targets' in f) return f.targets.slice(0, 5).map((t) => `${t.key}: ${maskQuery(t.url)}`).concat(f.targets.length > 5 ? [`… and ${f.targets.length - 5} more`] : [])
  return [`${f.urlSecret.origin} (the full address is the vault secret ${f.urlSecret.name})`]
}

function keepsLine(show: SourceShowResponse | null): string | null {
  const ex = show?.source.extract
  if (!ex) return null
  const parts: string[] = []
  const fields = Object.keys(ex.fields ?? {})
  if (fields.length) parts.push(`fields ${fields.join(', ')}`)
  if (ex.items) parts.push(`items${Object.keys(ex.items.itemFields ?? {}).length ? ` with ${Object.keys(ex.items.itemFields ?? {}).join(', ')}` : ''}`)
  const aggs = Object.keys(ex.aggregates ?? {})
  if (aggs.length) parts.push(`totals ${aggs.join(', ')}`)
  return parts.length ? parts.join('; ') : null
}

function Choice<T extends string>({
  label,
  value,
  options,
  onChange,
  testid,
}: {
  label: string
  value: T
  options: Array<{ value: T; label: string }>
  onChange: (v: T) => void
  testid: string
}): React.JSX.Element {
  return (
    <fieldset className="text-[11px]" data-testid={testid}>
      <legend className={muted}>{label}</legend>
      {options.map((o) => (
        <label key={o.value} className="flex items-center gap-1.5 text-[var(--color-text-secondary)]">
          <SquareRadio checked={value === o.value} onChange={() => onChange(o.value)} />
          {o.label}
        </label>
      ))}
    </fieldset>
  )
}

type Phase = { step: 'review' } | { step: 'confirm' } | { step: 'root'; hint: RootKeyHint; token: string } | { step: 'busy' }

export function DataSourceReview({
  scope,
  pending,
  onDone,
  onClose,
}: {
  scope: ServerScope
  pending: PendingSource & { maker?: { handle?: string | null; user?: string } }
  onDone: (msg: string) => void
  onClose: () => void
}): React.JSX.Element {
  const [show, setShow] = useState<SourceShowResponse | null>(null)
  const [tried, setTried] = useState<SourceTryResponse | null>(null)
  const [trying, setTrying] = useState(false)
  const [secrets, setSecrets] = useState<VaultSecret[]>([])
  const secretBound = pending.urlSecret != null
  const defaultsBySecret = pending.secret != null || pending.urlSecret != null
  const [secret, setSecret] = useState<string>(pending.urlSecret ?? pending.secret ?? '')
  const [viewers, setViewers] = useState<Viewers>(defaultsBySecret ? 'admin' : 'member')
  const [access, setAccess] = useState<AgentAccess>(defaultsBySecret ? 'shape' : 'values')
  const [keep, setKeep] = useState<Keep>('values')
  const [phase, setPhase] = useState<Phase>({ step: 'review' })
  const [error, setError] = useState<string | null>(null)
  const origin = pending.origins[0] ?? ''

  useEffect(() => {
    let live = true
    showSource(scope, pending.name)
      .then((s) => {
        if (!live) return
        setShow(s)
        setViewers(s.source.viewers ?? (defaultsBySecret ? 'admin' : 'member'))
        setAccess(s.source.agentAccess ?? (defaultsBySecret ? 'shape' : 'values'))
        setKeep(s.source.keep ?? 'values')
      })
      .catch(() => {
        /* the pending row alone is enough to review */
      })
    vaultApi
      .list()
      .then((rows) => live && setSecrets(rows))
      .catch(() => live && setSecrets([]))
    return () => {
      live = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scope, pending.name])

  const fitting = useMemo(() => secrets.filter((s) => secretFitsOrigin(s, origin)), [secrets, origin])

  const runTry = async (): Promise<void> => {
    setTrying(true)
    setError(null)
    try {
      setTried(await trySource(scope, { name: pending.name }))
    } catch (e) {
      setError(sourcesErrorText(e))
    } finally {
      setTrying(false)
    }
  }

  const approve = async (token?: string, useAnyway = false): Promise<void> => {
    setPhase({ step: 'busy' })
    setError(null)
    try {
      const r = await approveSource(scope, {
        name: pending.name,
        specHash: pending.specHash,
        ...(secret ? { secret } : {}),
        ...(token ? { confirmToken: token } : {}),
        viewers,
        agentAccess: access,
        keep,
        ...(useAnyway ? { useAnyway: true } : {}),
      })
      onDone(`${pending.label} is on${r.secretBound ? ` with ${secret}` : ''}.`)
    } catch (e) {
      const info = sourcesErrorOf(e)
      if (info?.code === 'root_key' && token && info.rootKey) {
        setPhase({ step: 'root', hint: info.rootKey as RootKeyHint, token })
        return
      }
      setPhase({ step: 'review' })
      setError(info?.code === 'spec_changed' ? 'It changed since you opened it. Close this and review it again.' : sourcesErrorText(e))
    }
  }

  const ownerOnly = pending.approval === 'owner_only'
  const by = makerLabel(pending.madeBy, pending.maker?.handle ?? pending.maker?.user ?? null)
  const lines = fetchLines(show)
  const keeps = keepsLine(show)

  return (
    <div className="space-y-2 border border-[var(--color-border)] p-2" data-testid="sources-review">
      <div className="text-[11px] space-y-0.5">
        <p className="text-[var(--color-text-primary)]">
          <strong>{pending.label}</strong> <span className={muted}>({pending.name})</span>
        </p>
        <p className={muted}>
          Proposed by {by} · every {every(pending.everySecs)} · {pending.targets} target{pending.targets === 1 ? '' : 's'} ·{' '}
          {pending.origins.join(', ')}
        </p>
        {lines.map((l) => (
          <p key={l} className="font-mono text-[10px] text-[var(--color-text-secondary)] break-all">
            {l}
          </p>
        ))}
        {keeps && <p className={muted}>Keeps: {keeps}</p>}
        {pending.lan && <p className={danger}>This reads a LAN address or plain http. Only the owner can approve it.</p>}
        {(show?.warnings ?? []).map((w) => (
          <p key={w.code + w.field} className="text-[var(--color-status-warn)]">
            {w.hint}
          </p>
        ))}
      </div>

      <div className="space-y-1">
        <button type="button" className={btn} onClick={() => void runTry()} disabled={trying} data-testid="sources-review-try">
          {trying ? 'Trying…' : 'Try it now'}
        </button>
        {tried && (
          <>
            <TrySummary r={tried} />
            <DataSourceTryTree nodes={tried.tree} truncated={tried.treeTruncated} />
          </>
        )}
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-3 gap-2">
        <Choice
          label="Who on this server can see it"
          value={viewers}
          onChange={setViewers}
          testid="sources-review-viewers"
          options={[
            { value: 'member', label: 'Members and up' },
            { value: 'admin', label: 'Admins only' },
          ]}
        />
        <Choice
          label="What agents see"
          value={access}
          onChange={setAccess}
          testid="sources-review-access"
          options={[
            { value: 'values', label: 'Values' },
            { value: 'shape', label: 'States only (values hidden; a guardrail)' },
          ]}
        />
        <Choice
          label="What the server keeps"
          value={keep}
          onChange={setKeep}
          testid="sources-review-keep"
          options={[
            { value: 'values', label: 'Values and states' },
            { value: 'states', label: 'States only (no values stored anywhere)' },
          ]}
        />
      </div>

      {!secretBound && (
        <div className="text-[11px] space-y-1">
          <label className={muted}>
            Secret{' '}
            <select
              className="ml-1 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)]"
              value={secret}
              onChange={(e) => setSecret(e.currentTarget.value)}
              data-testid="sources-review-secret"
            >
              <option value="">None</option>
              {pending.secret && !fitting.some((s) => s.name === pending.secret) && (
                <option value={pending.secret}>{pending.secret} (asked for)</option>
              )}
              {fitting.map((s) => (
                <option key={s.id} value={s.name}>
                  {s.name}
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            className={`${btn} ml-1.5`}
            onClick={() => useSettingsStore.getState().setSection('vault')}
            title={`Add a secret for ${origin} in Settings → Vault`}
          >
            Add a secret
          </button>
          {secret && <p className={muted}>K2 sends it only to {origin}. Agents never see it.</p>}
        </div>
      )}
      {secretBound && <p className={`text-[11px] ${muted}`}>Its address is the vault secret {pending.urlSecret}.</p>}

      {phase.step === 'confirm' && pending.id && (
        <ConfirmStep
          scope={scope}
          action="source-secret"
          secretId={pending.id}
          label={pending.name}
          onToken={(t) => void approve(t)}
          onCancel={() => setPhase({ step: 'review' })}
        />
      )}
      {phase.step === 'root' && (
        <div className="space-y-1 text-[11px]" data-testid="sources-review-rootkey">
          <p className="text-[var(--color-status-warn)]">{phase.hint.hint}</p>
          <div className="flex gap-1.5">
            <button type="button" className={btn} onClick={() => void approve(phase.token, true)} data-testid="sources-review-use-anyway">
              Use anyway
            </button>
            <button type="button" className={btn} onClick={() => setPhase({ step: 'review' })}>
              Pick another key
            </button>
          </div>
        </div>
      )}
      {(phase.step === 'review' || phase.step === 'busy') && (
        <div className="flex gap-1.5">
          <button
            type="button"
            className={btnPrimary}
            disabled={phase.step === 'busy'}
            onClick={() => (secret ? setPhase({ step: 'confirm' }) : void approve())}
            data-testid="sources-review-approve"
          >
            {secret ? 'Confirm and approve' : 'Approve'}
          </button>
          <button type="button" className={btn} onClick={onClose}>
            Close
          </button>
          {ownerOnly && <span className={`text-[10px] ${muted} self-center`}>Owner only</span>}
        </div>
      )}
      {error && (
        <p className={`text-[11px] ${danger}`} role="alert">
          {error}
        </p>
      )}
    </div>
  )
}
