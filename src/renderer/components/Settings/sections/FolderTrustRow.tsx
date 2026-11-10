// Settings → Workspaces → <workspace> → "Folder trust" (prd-security-sweep-
// 0464-v1 SS10). The daemon owns the decision (`/cli/folder-trust/*`).
// Settings shows where things stand, holds the undo (Reset: back to
// undecided, K2 asks again at the next start) and, while the folder waits
// for a decision, the same three choices as the Thread card (Rosson
// 2026-11-05): Trust it / Always trust this folder / Don't trust. They go
// to the same daemon decision with the same rule on who decides (the
// owner); whichever is used first, the card or this row, decides, the
// other updates (the card closes showing the choice; this row re-reads the
// status), and a refusal is shown here in the daemon's words.

import { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import type { ServerScope } from '@/kessel/server-scope'
import { Button } from '@/components/ui'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SettingSwitchRow } from '@/components/Settings/controls/SettingControls'

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
  /** The settings version the findings are (what a decision applies to). */
  fingerprint?: string | null
  trusted_by: string[]
  decide: string
  /** This caller may decide (the daemon's own check, the one `decide` runs).
   *  Absent from a daemon without it: no buttons offered there. */
  can_decide?: boolean
}

/** The decisions, as the card words them and the daemon names them. */
export const FOLDER_TRUST_CHOICES = [
  { label: 'Trust it', decision: 'once' },
  { label: 'Always trust this folder', decision: 'always' },
  { label: "Don't trust", decision: 'deny' },
] as const

/** States that still wait for a person's decision (the daemon's list). */
const UNDECIDED: readonly FolderTrustState[] = ['pending', 'changed', 'undecided']

/** How often the row re-reads the status while the folder waits, so a
 *  decision on the card (or another window) shows here too. */
export const FOLDER_TRUST_POLL_MS = 5000

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
      return 'No risky settings found in this folder'
    case 'trusted':
      return s.decided_by === 'upgrade' ? 'Trusted before K2 checked; K2 asks again if these settings change' : `Trusted this version${by}`
    case 'always':
      return `Always trusted${by}`
    case 'denied':
      return `Not trusted${by}: K2 starts no programs here`
    case 'pending':
      return 'Waiting for you: choose here or on the card in Thread'
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

/** The body of a refusal (`DaemonHttpError.body`), when it has one. */
function refusalBody(err: unknown): Record<string, unknown> | null {
  const body = err && typeof err === 'object' ? (err as { body?: unknown }).body : null
  return body && typeof body === 'object' && !Array.isArray(body) ? (body as Record<string, unknown>) : null
}

/** What a person reads for a refused decision: the folder-trust routes say
 *  it in `message` (`{error: code, message}`), the person check in the
 *  `{error: {code, hint}}` shape. */
export function folderTrustRefusal(err: unknown): string {
  const body = refusalBody(err)
  if (body && typeof body.message === 'string' && body.message.trim()) return body.message.trim()
  return daemonErrorText(err)
}

export interface FolderTrustControls {
  status: FolderTrustStatus | null
  /** The first read has answered (or failed). */
  loaded: boolean
  error: string | null
  notice: string | null
  busy: boolean
  /** The folder waits for a person's decision. */
  waiting: boolean
  decide: (decision: (typeof FOLDER_TRUST_CHOICES)[number]['decision']) => Promise<void>
  reset: () => Promise<void>
}

/** One folder's trust: the daemon's status and its two decisions. Settings'
 *  row and the chat's "this session can't start" panel both run on this, so
 *  they can't disagree. `enabled: false` reads nothing and forgets what it
 *  read; `pollDenied` also re-reads a denied folder (the panel shows the way
 *  out of it, so an undo elsewhere has to show). */
export function useFolderTrust(
  scope: ServerScope,
  projectPath: string,
  { enabled = true, pollDenied = false }: { enabled?: boolean; pollDenied?: boolean } = {},
): FolderTrustControls {
  const [status, setStatus] = useState<FolderTrustStatus | null>(null)
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // A read started before the last enable/disable or folder change is dropped.
  const readGen = useRef(0)

  const load = useCallback(async (): Promise<void> => {
    const gen = ++readGen.current
    try {
      const next = asStatus(await daemonCliGet<unknown>(scope, 'folder-trust/status', { path: projectPath }))
      if (gen !== readGen.current) return
      setStatus(next)
      setError(null)
    } catch (err) {
      if (gen !== readGen.current) return
      setError(err instanceof Error ? err.message : String(err))
    }
    setLoaded(true)
  }, [scope, projectPath])

  useEffect(() => {
    if (enabled) {
      void load()
      return
    }
    readGen.current += 1
    setStatus(null)
    setLoaded(false)
    setError(null)
    setNotice(null)
  }, [enabled, load])

  const waiting = status !== null && UNDECIDED.includes(status.state)
  const polling = enabled && (waiting || (pollDenied && status?.state === 'denied'))

  // While the folder waits: re-read now and then and when the window comes
  // back, so a decision made on the card shows here. A failed or odd read
  // keeps what is shown (never blanks the row).
  useEffect(() => {
    if (!polling) return
    let live = true
    const refresh = async (): Promise<void> => {
      try {
        const next = asStatus(await daemonCliGet<unknown>(scope, 'folder-trust/status', { path: projectPath }))
        if (live && next) setStatus(next)
      } catch {
        // The next tick tries again; a decision's own errors show inline.
      }
    }
    const timer = setInterval(() => void refresh(), FOLDER_TRUST_POLL_MS)
    const onFocus = (): void => void refresh()
    window.addEventListener('focus', onFocus)
    return () => {
      live = false
      clearInterval(timer)
      window.removeEventListener('focus', onFocus)
    }
  }, [polling, scope, projectPath])

  const reset = useCallback(async (): Promise<void> => {
    if (busy) return
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      setStatus(asStatus(await daemonCliPost<unknown>(scope, 'folder-trust/reset', { path: projectPath })))
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }, [busy, scope, projectPath])

  const decide = useCallback(
    async (decision: (typeof FOLDER_TRUST_CHOICES)[number]['decision']): Promise<void> => {
      if (busy || !status) return
      setBusy(true)
      setError(null)
      setNotice(null)
      try {
        // The same decision the card makes, for the version shown here;
        // `seen_state` lets the daemon refuse it when someone decided
        // first (the card, another window).
        const res = await daemonCliPost<{ message?: unknown; status?: unknown }>(scope, 'folder-trust/decide', {
          path: projectPath,
          decision,
          fingerprint: status.fingerprint ?? 'none',
          scope: status.scope,
          seen_state: status.state,
        })
        const next = asStatus(res?.status)
        if (next) setStatus(next)
        else await load()
        if (typeof res?.message === 'string' && res.message.trim()) setNotice(res.message.trim())
      } catch (err) {
        setError(folderTrustRefusal(err))
        const current = asStatus(refusalBody(err)?.current)
        if (current) setStatus(current)
      } finally {
        setBusy(false)
      }
    },
    [busy, status, scope, projectPath, load],
  )

  return { status, loaded, error, notice, busy, waiting, decide, reset }
}

/** What K2 found in the folder (and, when denied, what still trusts it). */
export function FolderTrustFindings({ status }: { status: FolderTrustStatus }): React.JSX.Element | null {
  const findings = status.findings.length > 0 && status.state !== 'clean' ? (
    <ul className={SETTING_ROW_HINT}>
      {status.findings.slice(0, 5).map((f) => (
        <li key={`${f.file}:${f.what}`}>
          <code>{f.file}</code>: {f.detail}
        </li>
      ))}
      {status.findings.length > 5 || status.more > 0 ? (
        <li>and {Math.max(0, status.findings.length - 5) + status.more} more</li>
      ) : null}
    </ul>
  ) : null
  const elsewhere = status.state === 'denied' && status.trusted_by.length > 0 ? (
    <div className={SETTING_ROW_HINT}>Still trusted elsewhere: {status.trusted_by.join('; ')}</div>
  ) : null
  if (!findings && !elsewhere) return null
  return (
    <>
      {findings}
      {elsewhere}
    </>
  )
}

export function FolderTrustRow({ scope, projectPath }: { scope: ServerScope; projectPath: string }): React.JSX.Element {
  const { status, error, notice, busy, waiting, decide, reset } = useFolderTrust(scope, projectPath)

  const canReset = status !== null && ['trusted', 'always', 'denied', 'changed'].includes(status.state)

  return (
    <SettingSwitchRow
      settingId="projects.folder-trust"
      control={
        canReset ? (
          <Button variant="quiet" onClick={() => void reset()} disabled={busy}>
            Reset
          </Button>
        ) : null
      }
    >
      <div className={SETTING_ROW_LABEL}>Folder trust</div>
      <div className={SETTING_ROW_HINT} data-testid="folder-trust-line">
        {status ? folderTrustLine(status) : error ? '' : 'Checking…'}
      </div>
      {status ? <FolderTrustFindings status={status} /> : null}
      {waiting ? (
        <div className="flex flex-wrap gap-1.5 mt-2" data-testid="folder-trust-choices">
          {FOLDER_TRUST_CHOICES.map((c) => (
            <Button key={c.decision} variant="quiet" onClick={() => void decide(c.decision)} disabled={busy}>
              {c.label}
            </Button>
          ))}
        </div>
      ) : null}
      {notice ? (
        <div className={SETTING_ROW_HINT} data-testid="folder-trust-notice">
          {notice}
        </div>
      ) : null}
      {error ? (
        <div role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-1">
          {error}
        </div>
      ) : null}
    </SettingSwitchRow>
  )
}
