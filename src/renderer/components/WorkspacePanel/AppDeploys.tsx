// App runtime stage 1 (prd-k2-app-runtime-v1 S8): the Published drawer's
// deploy history and Roll back button for an App row (`cmd === '(app)'`).
// Thin client: the daemon owns deploys, rollback and the audit; this only
// reads `GET /cli/app/history` and sends the `POST /cli/app/rollback`
// gesture, then re-reads.

import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { scopeMayWrite } from '@/kessel/server-scope'

/** `published_services.cmd` of an App runtime row (k2-core CMD_APP_SENTINEL). */
export const APP_CMD_SENTINEL = '(app)'

export interface AppDeployment {
  id: string
  kind: string
  fromSeed: string | null
  toSeed: string | null
  commitRef: string | null
  actor: string
  reason: string | null
  result: string
  error: string | null
  startedAt: number
}

export interface AppHistory {
  liveSeed: string | null
  deployments: AppDeployment[]
  auditVerified: boolean
}

function asHistory(raw: unknown): AppHistory {
  const v = (raw ?? {}) as Record<string, unknown>
  const list = Array.isArray(v.deployments) ? v.deployments : []
  return {
    liveSeed: typeof v.liveSeed === 'string' ? v.liveSeed : null,
    auditVerified: v.auditVerified === true,
    deployments: list.map((d) => {
      const r = (d ?? {}) as Record<string, unknown>
      const str = (k: string): string | null => (typeof r[k] === 'string' ? (r[k] as string) : null)
      return {
        id: str('id') ?? '',
        kind: str('kind') ?? '',
        fromSeed: str('fromSeed'),
        toSeed: str('toSeed'),
        commitRef: str('commitRef'),
        actor: str('actor') ?? '',
        reason: str('reason'),
        result: str('result') ?? '',
        error: str('error'),
        startedAt: typeof r.startedAt === 'number' ? r.startedAt : 0,
      }
    }),
  }
}

function short(h: string | null): string {
  return h ? h.slice(0, 12) : '—'
}

/** The newest earlier Seed that went live: what Roll back goes to. */
export function rollbackTarget(h: AppHistory): string | null {
  for (const d of h.deployments) {
    if (d.toSeed && d.toSeed !== h.liveSeed && ['live', 'superseded', 'rolled_back'].includes(d.result)) {
      return d.toSeed
    }
  }
  return null
}

export function AppDeploys({ scope, app }: { scope: ServerScope; app: string }): React.JSX.Element {
  const [history, setHistory] = useState<AppHistory | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const load = useCallback(async () => {
    try {
      const raw = await daemonCliGet<unknown>(scope, 'app/history', { app, limit: 10 })
      setHistory(asHistory(raw))
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [scope, app])

  useEffect(() => {
    void load()
  }, [load])

  const target = history ? rollbackTarget(history) : null
  const mayWrite = scopeMayWrite(scope, 'app/rollback')

  const rollBack = async (): Promise<void> => {
    setBusy(true)
    try {
      await daemonCliPost(scope, 'app/rollback', { app, wait: 30 })
      await load()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div data-app-deploys="" className="space-y-1.5">
      <div className="flex items-center justify-between">
        <div className="text-[10px] text-[var(--color-text-muted)]">Deploys</div>
        {mayWrite && target ? (
          <button
            type="button"
            disabled={busy}
            onClick={() => void rollBack()}
            className="px-2 py-0.5 text-[10px] border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] disabled:opacity-50"
            title={`Roll back to ${short(target)}`}
          >
            {busy ? 'Rolling back…' : 'Roll back'}
          </button>
        ) : null}
      </div>
      {error ? <div className="text-[10px] text-[var(--color-status-warn-text)]">{error}</div> : null}
      {history && history.deployments.length === 0 ? (
        <div className="text-[11px] text-[var(--color-text-muted)]">No deploys yet.</div>
      ) : null}
      {history?.deployments.map((d) => (
        <div key={d.id} data-deployment={d.id} className="text-[11px] text-[var(--color-text-primary)]">
          <span className="font-mono">{short(d.toSeed)}</span> {d.kind} · {d.result}
          {d.toSeed && d.toSeed === history.liveSeed && d.result === 'live' ? ' · live' : ''}
          <span className="block text-[10px] text-[var(--color-text-muted)]">
            by {d.actor}
            {d.commitRef ? ` · commit ${short(d.commitRef)}` : ''}
            {d.error ? ` · ${d.error}` : ''}
          </span>
        </div>
      ))}
      {history && !history.auditVerified ? (
        <div className="text-[10px] text-[var(--color-status-warn-text)]">The deploy audit chain does not verify.</div>
      ) : null}
    </div>
  )
}
