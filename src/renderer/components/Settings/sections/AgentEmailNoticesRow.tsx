// Settings → Workspaces → <workspace> → "Email notices" (prd-email-
// notifications-part2-v1 §4 N10): may this workspace's agent email the
// people on this server? Off by default. The owner turns it on with one of
// the server's senders and a fresh confirm (Touch ID / login, else the vault
// password), the same as `k2 notifications email agent on <workspace>
// --sender <address>`; off is `k2 notifications email agent off`. Admins see
// the state; only the owner changes it here. The daemon owns the rules
// (one owner per sender, the confirm, the budget); this row renders them.

import React, { useCallback, useEffect, useState } from 'react'
import { Toggle } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { fetchNotifyCaller } from './notify-api'
import {
  agentNoticesOff,
  agentNoticesOn,
  errorText,
  fetchAgentNotices,
  fetchNoticeSenderChoices,
  PasswordNeeded,
  type AgentNoticeRow,
  type NoticeSenderChoice,
} from './email-notice-api'

const muted = 'text-[10px] text-[var(--color-text-muted)]'

/** The row's one-line state. */
export function agentNoticeLine(row: AgentNoticeRow | null): string {
  if (!row || !row.sender) return 'Off. The agent can ask you with a permission card.'
  if (row.on) return `On: the agent emails people here from ${row.sender} (only people who confirmed an address).`
  return row.offReason ? `Off (${row.offReason}). Sender kept: ${row.sender}.` : `Off. Sender kept: ${row.sender}.`
}

export function AgentEmailNoticesRow({ scope, projectId }: { scope: ServerScope; projectId: string }): React.JSX.Element | null {
  const [row, setRow] = useState<AgentNoticeRow | null>(null)
  const [loaded, setLoaded] = useState(false)
  const [hidden, setHidden] = useState(false)
  const [isOwner, setIsOwner] = useState(false)
  const [choices, setChoices] = useState<NoticeSenderChoice[] | null>(null)
  const [sender, setSender] = useState('')
  const [askPassword, setAskPassword] = useState(false)
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async (): Promise<void> => {
    try {
      const rows = await fetchAgentNotices(scope)
      setRow(rows.find((r) => r.workspace === projectId) ?? null)
      setLoaded(true)
    } catch (e) {
      // Members can't read it (403); an older server has no route (404).
      if (e instanceof DaemonHttpError && (e.status === 403 || e.status === 404)) setHidden(true)
      else setError(errorText(e))
    }
  }, [scope, projectId])

  useEffect(() => {
    void load()
    let cancelled = false
    fetchNotifyCaller(scope).then(
      (c) => {
        if (!cancelled) setIsOwner(c.role === 'owner')
      },
      () => {
        if (!cancelled) setIsOwner(false)
      },
    )
    return () => {
      cancelled = true
    }
  }, [load, scope])

  if (hidden) return null

  const startOn = async (): Promise<void> => {
    setError(null)
    if (row?.sender) {
      setSender(row.sender)
      setChoices([])
      return
    }
    try {
      const c = await fetchNoticeSenderChoices(scope, projectId)
      setChoices(c)
      setSender(c[0]?.address ?? '')
    } catch (e) {
      setError(errorText(e))
    }
  }

  const confirmOn = async (pw?: string): Promise<void> => {
    if (!sender) return
    setBusy(true)
    setError(null)
    try {
      await agentNoticesOn(scope, projectId, sender, pw)
      setChoices(null)
      setAskPassword(false)
      setPassword('')
      await load()
    } catch (e) {
      if (e instanceof PasswordNeeded) setAskPassword(true)
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  const turnOff = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await agentNoticesOff(scope, projectId)
      await load()
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  const on = row?.on === true
  return (
    <div data-testid="agent-email-notices" data-settings-id="projects.agent-email-notices" className="py-2">
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          <div className="text-xs text-[var(--color-text-primary)]">Let this agent email people on this server</div>
          <div className={muted} data-testid="agent-email-notices-line">
            {loaded ? agentNoticeLine(row) : 'Loading…'}
          </div>
        </div>
        {isOwner && loaded && (
          <Toggle
            aria-label="Let this agent email people on this server"
            checked={on || choices !== null}
            disabled={busy}
            onChange={(next) => {
              if (next) void startOn()
              else if (choices !== null) setChoices(null)
              else void turnOff()
            }}
          />
        )}
      </div>
      {!isOwner && loaded && <p className={muted}>Only the owner turns this on or off.</p>}
      {isOwner && choices !== null && (
        <div data-testid="agent-email-notices-confirm" className="mt-2 space-y-2 border border-[var(--color-border)] p-2">
          {choices.length === 0 && !row?.sender ? (
            <p className={muted}>
              No free sender for this workspace yet. Add one first (from your own terminal): k2 notifications email sender
              add notices@your-domain
            </p>
          ) : (
            <>
              {row?.sender ? (
                <p className={muted}>Sends from {row.sender}.</p>
              ) : (
                <label className={`${muted} flex items-center gap-2`}>
                  Sends from
                  <select
                    value={sender}
                    disabled={busy}
                    onChange={(e) => setSender(e.target.value)}
                    aria-label="Sender for this workspace's notices"
                    className="px-1 py-0.5 text-xs bg-transparent border border-[var(--color-border)] text-[var(--color-text-primary)]"
                  >
                    {choices.map((c) => (
                      <option key={c.address} value={c.address}>
                        {c.address}
                        {c.state !== 'ready' ? ` (${c.state})` : ''}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              {askPassword && (
                <input
                  type="password"
                  value={password}
                  disabled={busy}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder="Vault password"
                  aria-label="Vault password"
                  className="w-full px-2 py-1 text-xs bg-transparent border border-[var(--color-border)] text-[var(--color-text-primary)]"
                />
              )}
              <div className="flex gap-2">
                <button
                  type="button"
                  disabled={busy || !sender || (askPassword && password === '')}
                  onClick={() => void confirmOn(askPassword ? password : undefined)}
                  className="px-2 py-1 text-[11px] border border-[var(--color-accent)] text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50"
                >
                  Confirm and turn on
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => {
                    setChoices(null)
                    setAskPassword(false)
                    setPassword('')
                  }}
                  className="px-2 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] no-drag cursor-pointer"
                >
                  Cancel
                </button>
              </div>
            </>
          )}
        </div>
      )}
      {error && (
        <p role="alert" data-testid="agent-email-notices-error" className="text-[11px] text-[var(--color-status-error)] mt-1">
          {error}
        </p>
      )}
    </div>
  )
}
