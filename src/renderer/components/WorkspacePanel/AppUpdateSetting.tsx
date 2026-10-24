// App runtime update continuity (0.46.4, Rosson 2026-10-24, Q4 + Q16): the
// App panel's per-App switch for K2 updates. When K2 updates, it moves each
// App to a fresh instance on the new runtime with no downtime. An App that
// can't run two copies at once (no room for both, extra ports, the process
// exception) either pauses briefly and updates (the default) or is skipped
// for the owner to update. Thin client: the daemon owns the policy
// (`GET /cli/app/rehost?app=`, `POST /cli/app/rehost/policy`).

import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { scopeMayWrite } from '@/kessel/server-scope'

export type NoOverlap = 'pause' | 'defer'

export interface AppRehost {
  whenNoOverlap: NoOverlap
  policy: string
  /** What the wave said about this App, in words (null when nothing). */
  wave: string | null
}

export function parseRehost(raw: unknown): AppRehost | null {
  const v = (raw ?? {}) as Record<string, unknown>
  const r = (v.rehost ?? null) as Record<string, unknown> | null
  if (!r || (r.whenNoOverlap !== 'pause' && r.whenNoOverlap !== 'defer')) return null
  const w = (r.wave ?? null) as Record<string, unknown> | null
  const wave =
    w && typeof w.state === 'string' && w.state !== 'current'
      ? `${w.state}${typeof w.detail === 'string' && w.detail ? `: ${w.detail}` : ''}`
      : null
  return { whenNoOverlap: r.whenNoOverlap, policy: typeof r.policy === 'string' ? r.policy : 'auto', wave }
}

const CHOICES: { value: NoOverlap; label: string }[] = [
  { value: 'pause', label: 'Pause briefly and update' },
  { value: 'defer', label: 'Skip it and let me update it myself' },
]

export function AppUpdateSetting({ scope, app }: { scope: ServerScope; app: string }): React.JSX.Element | null {
  const [state, setState] = useState<AppRehost | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const load = useCallback(async () => {
    try {
      setState(parseRehost(await daemonCliGet<unknown>(scope, 'app/rehost', { app })))
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [scope, app])

  useEffect(() => {
    void load()
  }, [load])

  const mayWrite = scopeMayWrite(scope, 'app/rehost/policy')

  const choose = async (value: NoOverlap): Promise<void> => {
    if (!state || value === state.whenNoOverlap) return
    const before = state
    setState({ ...state, whenNoOverlap: value })
    setBusy(true)
    try {
      const raw = await daemonCliPost<unknown>(scope, 'app/rehost/policy', { app, whenNoOverlap: value })
      setState(parseRehost(raw) ?? { ...before, whenNoOverlap: value })
      setError(null)
    } catch (e) {
      setState(before)
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  // An older daemon (no re-host route) or a failed read: nothing to show.
  if (!state) return null
  return (
    <div data-app-update-setting="" className="space-y-1">
      <div className="text-[10px] text-[var(--color-text-muted)]">When K2 updates and this App can&apos;t run two copies at once</div>
      {state ? (
        <div role="radiogroup" aria-label="When an update can't run two copies" className="space-y-0.5">
          {CHOICES.map((c) => (
            <label key={c.value} className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-primary)]">
              <input
                type="radio"
                name={`rehost-${app}`}
                value={c.value}
                checked={state.whenNoOverlap === c.value}
                disabled={!mayWrite || busy}
                onChange={() => void choose(c.value)}
              />
              {c.label}
            </label>
          ))}
          {state.wave ? <div className="text-[10px] text-[var(--color-text-muted)]">Last update: {state.wave}</div> : null}
        </div>
      ) : null}
      {error ? <div className="text-[10px] text-[var(--color-status-warn-text)]">{error}</div> : null}
    </div>
  )
}
