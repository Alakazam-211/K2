// Settings → Workspaces → <workspace> → "Folder trust" (prd-security-sweep-
// 0464-v1 SS10). The daemon owns the decision (`/cli/folder-trust/*`); the
// owner decides on the Thread card. Settings shows where things stand and
// holds the undo: Reset returns the folder to undecided, and K2 asks again
// at the next start (feedback_cards_not_settings_hunts: decide on the card,
// undo here).

import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

export type FolderTrustState = 'clean' | 'trusted' | 'always' | 'denied' | 'pending' | 'undecided' | 'changed'

export interface FolderTrustFinding {
  tool: string
  file: string
  what: string
  detail: string
}

export interface FolderTrustStatus {
  path: string
  scope: string
  state: FolderTrustState
  decided_by?: string | null
  decided_at?: number | null
  findings: FolderTrustFinding[]
  more: number
  trusted_by: string[]
  decide: string
}

function day(at: number | null | undefined): string {
  if (!at) return ''
  const d = new Date(at * 1000)
  return `${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

/** The one line Settings shows for a folder's trust (PRD §3.4). */
export function folderTrustLine(s: FolderTrustStatus): string {
  const by = s.decided_by ? ` by ${s.decided_by}${s.decided_at ? ` on ${day(s.decided_at)}` : ''}` : ''
  switch (s.state) {
    case 'clean':
      return 'Clean, no risky settings'
    case 'trusted':
      return s.decided_by === 'upgrade' ? 'Trusted before K2 checked; K2 asks again if these settings change' : `Trusted this version${by}`
    case 'always':
      return `Always${by}`
    case 'denied':
      return `Not trusted${by}: K2 starts no programs here`
    case 'pending':
      return 'Waiting for you: answer the card on Thread'
    case 'changed':
      return 'The settings changed since you trusted them; K2 asks at the next start'
    case 'undecided':
      return 'Has risky settings; K2 asks at the next start'
  }
}

/** A daemon answer with the shape this row reads (an older daemon without
 *  the route, or a test double, gives something else). */
function asStatus(v: unknown): FolderTrustStatus | null {
  if (!v || typeof v !== 'object') return null
  const s = v as Partial<FolderTrustStatus>
  if (typeof s.state !== 'string' || typeof s.path !== 'string') return null
  return { ...s, findings: Array.isArray(s.findings) ? s.findings : [], trusted_by: Array.isArray(s.trusted_by) ? s.trusted_by : [], more: s.more ?? 0 } as FolderTrustStatus
}

export function FolderTrustRow({ scope, projectPath }: { scope: ServerScope; projectPath: string }): React.JSX.Element {
  const [status, setStatus] = useState<FolderTrustStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const load = useCallback(async (): Promise<void> => {
    try {
      setStatus(asStatus(await daemonCliGet<unknown>(scope, 'folder-trust/status', { path: projectPath })))
      setError(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [scope, projectPath])

  useEffect(() => {
    void load()
  }, [load])

  const reset = useCallback(async (): Promise<void> => {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      setStatus(asStatus(await daemonCliPost<unknown>(scope, 'folder-trust/reset', { path: projectPath })))
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }, [busy, scope, projectPath])

  const canReset = status !== null && ['trusted', 'always', 'denied', 'changed'].includes(status.state)

  return (
    <div className="border border-[var(--color-border)] p-3" data-settings-id="projects.folder-trust">
      <div className="flex items-start gap-3">
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">Folder trust</div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed" data-testid="folder-trust-line">
            {status ? folderTrustLine(status) : error ? '' : 'Checking…'}
          </div>
          {status && status.findings.length > 0 && status.state !== 'clean' ? (
            <ul className="text-[10px] text-[var(--color-text-secondary)] mt-1 leading-relaxed">
              {status.findings.slice(0, 5).map((f) => (
                <li key={`${f.file}:${f.what}`}>
                  <code>{f.file}</code>: {f.detail}
                </li>
              ))}
              {status.findings.length > 5 || status.more > 0 ? (
                <li>and {Math.max(0, status.findings.length - 5) + status.more} more</li>
              ) : null}
            </ul>
          ) : null}
          {status && status.state === 'denied' && status.trusted_by.length > 0 ? (
            <div className="text-[10px] text-[var(--color-text-muted)] mt-1">Still trusted elsewhere: {status.trusted_by.join('; ')}</div>
          ) : null}
          {error ? (
            <div role="alert" className="text-[10px] text-[var(--color-danger,#c33)] mt-1">
              {error}
            </div>
          ) : null}
        </div>
        {canReset ? (
          <button
            type="button"
            onClick={() => void reset()}
            disabled={busy}
            className="text-[10px] px-2 py-1 border border-[var(--color-border)] no-drag cursor-pointer disabled:opacity-50"
          >
            Reset
          </button>
        ) : null}
      </div>
    </div>
  )
}
