// Settings → Cores → App Approvals (prd-k2-app-runtime-v1 §5.6, 0.46.1 door).
//
// Agents propose, the owner approves in person. Every held App request
// (config, publish run, App key, clean mode, egress, deploy/install caps,
// raw database, overcommit, manage, quota) waits here in plain words:
//   - Approve = a fresh vault owner confirm for THAT request (action
//     `app-approve`, `secretId` = the request id; Touch ID / login password
//     on the Mac, the Connect or vault password elsewhere), then
//     POST /cli/app/approve with the one-use token.
//   - Deny = POST /cli/app/deny with an optional note the requester reads.
// AR43: with no vault password and no Owner Connect login the daemon can't
// take a confirm, so Approve is off and the daemon's hint shows.
// Thin client: no polling; it reloads on open, after a gesture, when the
// window regains focus, and on the daemon's `app_approvals_changed` push
// (0.46.4: a request an agent files while this is open shows at once).

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Callout } from '@/components/ui'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { daemonErrorText } from '@/lib/daemon-error'
import type { SettingEntry } from '../searchManifest'
import { vaultApi } from './vault-api'
import { useAppApprovalsStore } from '@/stores/app-approvals'
import {
  APP_APPROVE_CONFIRM_ACTION,
  approveAppRequest,
  capsAfter,
  denyAppRequest,
  diffLines,
  expiryText,
  fetchAppApprovals,
  kindLabel,
  resultError,
  type AppRequest,
  type OwnerConfirmStatus,
} from './app-approvals-api'

export const APP_APPROVALS_MANIFEST: SettingEntry[] = [
  {
    id: 'app-approvals.list',
    section: 'app-approvals',
    label: 'App Approvals',
    description: 'Held App requests (deploy caps, egress, quotas, keys): approve in person or deny',
    keywords: ['approvals', 'approve', 'deny', 'held', 'app', 'apps', 'deploy', 'install', 'caps', 'egress', 'quota', 'request'],
  },
]

const DENY_MAX = 500
const NO_CONFIRM_HINT =
  'No vault password and no Owner Connect login on this box: App approvals stay held until one is set (k2 vault password set).'
const muted = 'text-[var(--color-text-muted)]'
const secondary = 'text-[var(--color-text-secondary)]'

function nowSec(): number {
  return Math.floor(Date.now() / 1000)
}

export function AppApprovalsSection(): React.JSX.Element {
  const [rows, setRows] = useState<AppRequest[] | null>(null)
  const [ownerConfirm, setOwnerConfirm] = useState<OwnerConfirmStatus | null>(null)
  const [showHistory, setShowHistory] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<Notice | null>(null)

  const load = useCallback(async () => {
    try {
      const r = await fetchAppApprovals(showHistory ? 'all' : 'pending')
      setRows(r.requests)
      setOwnerConfirm(r.ownerConfirm)
      setError(null)
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }, [showHistory])

  useEffect(() => {
    void load()
    const onFocus = (): void => void load()
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [load])

  // The daemon's push (a request filed, asked again or decided).
  const revision = useAppApprovalsStore((s) => s.revision)
  const firstRevision = useRef(revision)
  useEffect(() => {
    if (revision !== firstRevision.current) void load()
  }, [revision, load])

  const pending = (rows ?? []).filter((r) => r.status === 'pending')
  const history = (rows ?? []).filter((r) => r.status !== 'pending')
  const canConfirm = ownerConfirm?.available === true

  return (
    <div className="max-w-3xl space-y-4" data-testid="app-approvals" data-settings-id="app-approvals.list">
      <div>
        <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
          App Approvals{pending.length > 0 ? ` (${pending.length})` : ''}
        </h2>
        <p className={`text-[11px] ${muted} mt-1 leading-relaxed`}>
          Changes to an App that only the server&apos;s owner may allow wait here: new caps on a deploy or install, outside
          hosts, raised quotas, keys and more. Agents can only ask. You approve each one in person, with Touch ID or your
          password. Requests expire after 7 days.
        </p>
      </div>
      {ownerConfirm && !canConfirm ? (
        <Callout tone="warn" data-testid="app-approvals-no-confirm">
          {ownerConfirm.hint || NO_CONFIRM_HINT}
        </Callout>
      ) : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
      {notice ? (
        <Callout tone={notice.tone} data-testid="app-approvals-notice">
          {notice.text}
        </Callout>
      ) : null}
      {rows && pending.length === 0 ? <p className={`text-[11px] ${muted}`}>Nothing is waiting.</p> : null}
      {pending.map((r) => (
        <RequestCard
          key={r.id}
          request={r}
          canConfirm={canConfirm}
          onDone={async (note) => {
            setNotice(note)
            await load()
          }}
        />
      ))}
      <div className="pt-2 border-t border-[var(--color-border)] space-y-2">
        <button
          type="button"
          className={`text-[11px] ${secondary} underline no-drag cursor-pointer`}
          onClick={() => setShowHistory((v) => !v)}
          data-testid="app-approvals-history-toggle"
        >
          {showHistory ? 'Hide history' : 'Show history'}
        </button>
        {showHistory && rows ? (
          history.length === 0 ? (
            <p className={`text-[11px] ${muted}`}>No decided requests yet.</p>
          ) : (
            <div className="space-y-2" data-testid="app-approvals-history">
              {history.map((r) => (
                <HistoryRow key={r.id} request={r} />
              ))}
            </div>
          )
        ) : null}
      </div>
    </div>
  )
}

function KindBadge({ kind }: { kind: string }): React.JSX.Element {
  return (
    <span className="text-[9px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
      {kindLabel(kind)}
    </span>
  )
}

function AppLine({ r }: { r: AppRequest }): React.JSX.Element {
  return (
    <div className="flex flex-wrap items-center gap-2">
      <KindBadge kind={r.kind} />
      <span className="text-xs font-mono text-[var(--color-text-primary)]">{r.appLabel}</span>
      {r.workspaceHandle ? <span className={`text-[11px] ${muted}`}>{r.workspaceHandle}</span> : null}
    </div>
  )
}

/** What the last gesture did. An approval whose apply failed leaves the
 *  pending list (it is decided), so its error shows here and in History. */
type Notice = { tone: 'ok' | 'error'; text: string }

type ConfirmStep = { step: 'idle' } | { step: 'starting' } | { step: 'mac' } | { step: 'password'; method: string }

function RequestCard({
  request: r,
  canConfirm,
  onDone,
}: {
  request: AppRequest
  canConfirm: boolean
  onDone: (notice: Notice) => Promise<void>
}): React.JSX.Element {
  const [confirm, setConfirm] = useState<ConfirmStep>({ step: 'idle' })
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [denying, setDenying] = useState(false)
  const [note, setNote] = useState('')
  const pwRef = useRef<HTMLInputElement>(null)
  const [hasPw, setHasPw] = useState(false)
  const lines = diffLines(r)
  const after = capsAfter(r)
  const now = nowSec()

  const finish = async (method: string, password?: string): Promise<void> => {
    const { token } = await vaultApi.confirmFinish({
      action: APP_APPROVE_CONFIRM_ACTION,
      secretId: r.id,
      method,
      label: `${r.appLabel}: ${r.summary}`.slice(0, 64),
      ...(password !== undefined ? { password } : {}),
    })
    const out = await approveAppRequest(r.id, token)
    setConfirm({ step: 'idle' })
    if (out.error) {
      // Recorded as approved, but applying it failed: the daemon says why.
      await onDone({
        tone: 'error',
        text: `Approved ${kindLabel(r.kind).toLowerCase()} for ${r.appLabel}, but applying it failed: ${out.error.hint || out.error.code}`,
      })
      return
    }
    await onDone({ tone: 'ok', text: `Approved ${kindLabel(r.kind).toLowerCase()} for ${r.appLabel}: ${out.note ?? 'it applies now'}` })
  }

  const approve = async (): Promise<void> => {
    setError(null)
    setConfirm({ step: 'starting' })
    try {
      const { methods } = await vaultApi.confirmStart(APP_APPROVE_CONFIRM_ACTION, r.id)
      if (methods.includes('local-auth')) {
        setConfirm({ step: 'mac' })
        await finish('local-auth')
        return
      }
      const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
      if (pw) {
        setConfirm({ step: 'password', method: pw })
        return
      }
      setConfirm({ step: 'idle' })
      setError(NO_CONFIRM_HINT)
    } catch (e) {
      setConfirm({ step: 'idle' })
      setError(daemonErrorText(e))
    }
  }

  const submitPassword = async (method: string): Promise<void> => {
    const el = pwRef.current
    if (!el) return
    const pw = el.value
    el.value = ''
    setHasPw(false)
    setError(null)
    try {
      await finish(method, pw)
    } catch (e) {
      setConfirm({ step: 'idle' })
      setError(daemonErrorText(e))
    }
  }

  const deny = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await denyAppRequest(r.id, note.trim() || undefined)
      setDenying(false)
      await onDone({ tone: 'ok', text: `Denied ${kindLabel(r.kind).toLowerCase()} for ${r.appLabel}.` })
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="border border-[var(--color-border)] p-3 space-y-2" data-testid={`app-request-${r.id}`}>
      <AppLine r={r} />
      <div className="text-xs text-[var(--color-text-primary)]">{r.summary}</div>
      {lines.length > 0 ? (
        <ul
          className="text-[11px] font-mono list-none space-y-0.5 border border-[var(--color-border)] p-2 text-[var(--color-text-primary)]"
          data-testid="app-request-diff"
        >
          {lines.map((l, i) => (
            <li key={i}>{l}</li>
          ))}
        </ul>
      ) : null}
      {after.length > 0 ? (
        <div className={`text-[10px] ${muted}`}>After approval it holds: {after.join(', ')}</div>
      ) : null}
      <div className={`text-[11px] ${secondary} flex flex-wrap gap-x-3`}>
        <span>asked by {r.requestedBy}</span>
        <span>{formatRelativeTime(r.requestedAt, now)}</span>
        <span className={muted}>{expiryText(r.expiresAt, now)}</span>
      </div>
      {r.reason ? <div className={`text-[11px] ${secondary}`}>Why: {r.reason}</div> : null}
      {r.reasons.length > 0 ? (
        <div className={`text-[11px] ${muted}`} data-testid="app-request-reasons">
          Held because: {r.reasons.join('; ')}
        </div>
      ) : null}

      {confirm.step === 'starting' ? <p className={`text-[11px] ${muted}`}>Asking for your confirm…</p> : null}
      {confirm.step === 'mac' ? <p className={`text-[11px] ${muted}`}>Confirm on your Mac.</p> : null}
      {confirm.step === 'password' ? (
        <form
          className="flex flex-col gap-1.5 max-w-sm"
          onSubmit={(e) => {
            e.preventDefault()
            void submitPassword(confirm.method)
          }}
        >
          <label className={`text-[11px] ${muted}`}>
            {confirm.method === 'connect-password' ? 'Your K2 Connect password' : 'Your vault password'}
            <input
              ref={pwRef}
              type="password"
              autoComplete="current-password"
              data-testid="app-approve-password"
              onChange={(e) => setHasPw(e.currentTarget.value !== '')}
              className="mt-1 w-full border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs"
            />
          </label>
          <div className="flex gap-1.5">
            <Button type="submit" variant="accent" disabled={!hasPw}>
              Approve
            </Button>
            <Button type="button" variant="ghost" onClick={() => setConfirm({ step: 'idle' })}>
              Cancel
            </Button>
          </div>
        </form>
      ) : null}

      {confirm.step === 'idle' ? (
        denying ? (
          <div className="flex flex-col gap-1.5 max-w-md">
            <textarea
              value={note}
              maxLength={DENY_MAX}
              rows={2}
              onChange={(e) => setNote(e.target.value.slice(0, DENY_MAX))}
              placeholder="Why (optional; the requester reads this)"
              aria-label="Why deny"
              data-testid={`app-deny-note-${r.id}`}
              className="w-full border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs text-[var(--color-text-primary)]"
            />
            <div className="flex items-center gap-2">
              <Button disabled={busy} onClick={() => void deny()} data-testid={`app-deny-send-${r.id}`}>
                Deny
              </Button>
              <Button variant="ghost" onClick={() => setDenying(false)}>
                Cancel
              </Button>
              <span className={`text-[10px] ${muted}`}>
                {note.length}/{DENY_MAX}
              </span>
            </div>
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="accent"
              disabled={busy || !canConfirm}
              title={canConfirm ? undefined : 'No way to confirm on this box yet'}
              onClick={() => void approve()}
              data-testid={`app-approve-${r.id}`}
            >
              Approve
            </Button>
            <Button disabled={busy} onClick={() => setDenying(true)} data-testid={`app-deny-${r.id}`}>
              Deny
            </Button>
          </div>
        )
      ) : null}
      {error ? (
        <Callout tone="error" data-testid={`app-request-error-${r.id}`}>
          {error}
        </Callout>
      ) : null}
    </div>
  )
}

const STATUS_WORDS: Record<string, string> = {
  approved: 'Approved',
  denied: 'Denied',
  withdrawn: 'Withdrawn',
  expired: 'Expired',
  failed: 'Approved, failed to apply',
}

function HistoryRow({ request: r }: { request: AppRequest }): React.JSX.Element {
  const failed = r.status === 'failed' ? resultError(r) : null
  const now = nowSec()
  return (
    <div className="border border-[var(--color-border)] p-2 space-y-1" data-testid={`app-history-${r.id}`}>
      <AppLine r={r} />
      <div className={`text-[11px] ${secondary}`}>{r.summary}</div>
      <div className={`text-[11px] ${muted} flex flex-wrap gap-x-3`}>
        <span className={r.status === 'approved' ? 'text-[var(--color-status-ok-soft)]' : undefined}>
          {STATUS_WORDS[r.status] ?? r.status}
        </span>
        {r.decidedBy ? <span>by {r.decidedBy}</span> : null}
        {r.confirmVia ? <span>via {r.confirmVia}</span> : null}
        {r.decidedAt ? <span>{formatRelativeTime(r.decidedAt, now)}</span> : null}
        <span>asked by {r.requestedBy}</span>
      </div>
      {r.decisionReason ? <div className={`text-[11px] ${secondary}`}>Note: {r.decisionReason}</div> : null}
      {failed ? <div className="text-[11px] text-[var(--color-status-error-soft)]">{failed}</div> : null}
    </div>
  )
}
