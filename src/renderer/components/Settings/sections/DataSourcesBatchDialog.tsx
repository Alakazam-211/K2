// Review N pending (contract-sources-v1 §4.3, §8; PRD §9.5, Q12). Owner: B2.
//
// One dialog for the pending sources and triggers a Garden (or this server)
// names, grouped by server. Secretless items need no confirm; the
// secret-bound items on one server share ONE `source-batch` confirm bound to
// the hash of their sorted `<sourceId>:<secretName>` lines [GV43]. Each
// server is approved in its own call (one transaction there); a changed list
// is refused (409 `batch_changed`) and the dialog reloads that server.
//
// `openSourcesBatchReview({gardenName, byServer})` is the entry point B3's
// Garden "Review N pending" calls. It mounts its own root, so no host
// component has to be placed anywhere.

import React, { useCallback, useEffect, useState } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import type { ServerScope } from '@/kessel/server-scope'
import type { PendingSource, PendingTrigger } from '@/lib/sources/sources-types'
import { batchApproveSources, pendingSources } from '@/lib/sources/sources-api'
import { registerZenSourcesBatchReview } from '@/lib/zen/zen-sources-review'
import { ConfirmStep } from './DataSourceConfirm'
import { batchHash, btn, btnPrimary, danger, every, makerLabel, muted, onLabel, scopeForHostKey, sourcesErrorText } from './data-sources-ui'
import { SquareCheckbox } from '@/components/ui'

export interface SourcesBatchArgs {
  /** Shown in the title: Review pending for Garden "Fleet". */
  gardenName?: string
  /** Host key (`local`, `akzm.k2.dev`, …) → source names on that server. */
  byServer: Record<string, string[]>
}

type ServerState =
  | { step: 'loading' }
  | { step: 'ready'; sources: PendingSource[]; triggers: PendingTrigger[] }
  | { step: 'confirm'; sources: PendingSource[]; triggers: PendingTrigger[]; hash: string; label: string }
  | { step: 'done'; text: string }
  | { step: 'failed'; text: string }

function ServerBlock({
  hostKey,
  names,
  scopeOf,
}: {
  hostKey: string
  names: string[]
  scopeOf: (hostKey: string) => ServerScope
}): React.JSX.Element {
  const scope = scopeOf(hostKey)
  const [state, setState] = useState<ServerState>({ step: 'loading' })
  const [picked, setPicked] = useState<Record<string, boolean>>({})
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(() => {
    setState({ step: 'loading' })
    pendingSources(scope, names.length ? names : undefined)
      .then((r) => {
        setState({ step: 'ready', sources: r.sources, triggers: r.triggers })
        const all: Record<string, boolean> = {}
        for (const s of r.sources) all[`s:${s.name}`] = s.approval !== 'owner_only'
        for (const t of r.triggers) all[`t:${t.id}`] = true
        setPicked(all)
      })
      .catch((e) => setState({ step: 'failed', text: sourcesErrorText(e) }))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hostKey, names.join(',')])

  useEffect(load, [load])

  const send = async (sources: PendingSource[], triggers: PendingTrigger[], token?: string): Promise<void> => {
    setError(null)
    try {
      const r = await batchApproveSources(scope, {
        items: sources.map((s) => ({
          name: s.name,
          specHash: s.specHash,
          ...((s.urlSecret ?? s.secret) ? { secret: (s.urlSecret ?? s.secret) as string } : {}),
        })),
        triggers: triggers.map((t) => t.id),
        ...(token ? { confirmToken: token } : {}),
      })
      const n = r.approved.length + r.triggersApproved.length
      setState({ step: 'done', text: `Approved ${n} on ${scope.label}.` })
    } catch (e) {
      setError(sourcesErrorText(e))
      load()
    }
  }

  const approveAll = async (): Promise<void> => {
    if (state.step !== 'ready') return
    const sources = state.sources.filter((s) => picked[`s:${s.name}`])
    const triggers = state.triggers.filter((t) => picked[`t:${t.id}`])
    if (sources.length + triggers.length === 0) return
    const pairs = sources
      .filter((s) => (s.urlSecret ?? s.secret) && s.id)
      .map((s) => ({ id: s.id as string, secret: (s.urlSecret ?? s.secret) as string }))
    if (pairs.length === 0) {
      await send(sources, triggers)
      return
    }
    const hash = await batchHash(pairs)
    setState({ step: 'confirm', sources, triggers, hash, label: `${pairs.length} source secret${pairs.length === 1 ? '' : 's'}` })
  }

  return (
    <div className="space-y-1.5 border border-[var(--color-border)] p-2" data-testid="sources-batch-server">
      <p className="text-[11px] text-[var(--color-text-primary)]">
        On <strong>{scope.label}</strong>
      </p>
      {state.step === 'loading' && <p className={`text-[11px] ${muted}`}>Loading…</p>}
      {state.step === 'failed' && <p className={`text-[11px] ${danger}`}>{state.text}</p>}
      {state.step === 'done' && <p className={`text-[11px] ${muted}`}>{state.text}</p>}
      {state.step === 'ready' && state.sources.length + state.triggers.length === 0 && (
        <p className={`text-[11px] ${muted}`}>Nothing is waiting here.</p>
      )}
      {state.step === 'ready' && (
        <>
          {state.sources.map((s) => {
            const sec = s.urlSecret ?? s.secret
            const key = `s:${s.name}`
            return (
              <label key={key} className="flex items-start gap-1.5 text-[11px] text-[var(--color-text-secondary)]">
                <SquareCheckbox
                  disabled={s.approval === 'owner_only'}
                  checked={picked[key] ?? false}
                  onChange={(e) => {
                    const on = e.currentTarget.checked
                    setPicked((p) => ({ ...p, [key]: on }))
                  }}
                  data-testid="sources-batch-item"
                />
                <span>
                  <strong>{s.label}</strong> · every {every(s.everySecs)} · {s.targets} target{s.targets === 1 ? '' : 's'} ·{' '}
                  {s.origins.join(', ')} · by {makerLabel(s.madeBy)}
                  {sec && ` · uses ${sec}`}
                  {s.approval === 'owner_only' && ' · owner only (approve it alone)'}
                </span>
              </label>
            )
          })}
          {state.triggers.map((t) => {
            const key = `t:${t.id}`
            return (
              <label key={key} className="flex items-start gap-1.5 text-[11px] text-[var(--color-text-secondary)]">
                <SquareCheckbox
                  checked={picked[key] ?? false}
                  onChange={(e) => {
                    const on = e.currentTarget.checked
                    setPicked((p) => ({ ...p, [key]: on }))
                  }}
                  data-testid="sources-batch-item"
                />
                <span>
                  Trigger: when {t.source} {onLabel(t.on)}, wake <strong>{t.agent.handle}</strong>
                </span>
              </label>
            )
          })}
          {state.sources.length + state.triggers.length > 0 && (
            <button type="button" className={btnPrimary} onClick={() => void approveAll()} data-testid="sources-batch-approve">
              Approve all ticked
            </button>
          )}
        </>
      )}
      {state.step === 'confirm' && (
        <ConfirmStep
          scope={scope}
          action="source-batch"
          secretId={state.hash}
          label={state.label}
          onToken={(t) => void send(state.sources, state.triggers, t)}
          onCancel={load}
        />
      )}
      {error && (
        <p className={`text-[11px] ${danger}`} role="alert">
          {error}
        </p>
      )}
    </div>
  )
}

export function DataSourcesBatchDialog({
  args,
  onClose,
  scopeOf = scopeForHostKey,
}: {
  args: SourcesBatchArgs
  onClose: () => void
  scopeOf?: (hostKey: string) => ServerScope
}): React.JSX.Element {
  const servers = Object.entries(args.byServer)
  return (
    <div className="fixed inset-0 z-[400] flex items-center justify-center bg-black/40" data-testid="sources-batch">
      <div className="bg-[var(--color-bg)] border border-[var(--color-border)] p-3 w-[560px] max-w-[95vw] max-h-[85vh] overflow-auto space-y-2">
        <p className="text-[12px] font-medium text-[var(--color-text-primary)]">
          {args.gardenName ? `Review pending for Garden “${args.gardenName}”` : 'Review pending'}
        </p>
        <p className={`text-[11px] ${muted}`}>
          Each check runs on its own server. Items that use a secret on one server need one confirm.
        </p>
        {servers.map(([hostKey, names]) => (
          <ServerBlock key={hostKey} hostKey={hostKey} names={names} scopeOf={scopeOf} />
        ))}
        <div className="flex justify-end">
          <button type="button" className={btn} onClick={onClose} data-testid="sources-batch-close">
            Close
          </button>
        </div>
      </div>
    </div>
  )
}

let root: Root | null = null
let host: HTMLElement | null = null

/** Open the batch review (B3's Garden "Review N pending" entry point). */
export function openSourcesBatchReview(args: SourcesBatchArgs): void {
  if (typeof document === 'undefined') return
  if (!host) {
    host = document.createElement('div')
    host.setAttribute('data-k2-sources-batch', '')
    document.body.appendChild(host)
  }
  if (!root) root = createRoot(host)
  const close = (): void => {
    root?.render(<></>)
  }
  root.render(<DataSourcesBatchDialog args={args} onClose={close} />)
}

// B3's Garden pill ("Review N pending") opens this dialog. Registered at
// module load: the app imports Settings (and so this file) at start, and the
// Garden never imports Settings.
registerZenSourcesBatchReview(openSourcesBatchReview)
