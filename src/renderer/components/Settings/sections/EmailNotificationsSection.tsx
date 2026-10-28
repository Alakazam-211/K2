// Settings → Cores → Email Notifications (prd-email-notifications-part2-v1
// §4 N8). The owner's view of the mail K2 sends from senders (resets,
// codes, App email, agent notices): the senders and their pauses, the
// workspaces whose agents may email people, suppressed addresses and
// the recent log. Every rule lives in the daemon; this page only reads
// `/cli/notifications/email/*` and sends the owner's gestures (resume
// with a confirm, turn a workspace off, remove a suppression).
// Adding senders and relays stays on the CLI for now
// (`k2 notifications email --help`).

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { SettingEntry } from '../searchManifest'
import {
  emailNotifyApi,
  type EmailNotifyApi,
  notifyErrorMessage,
  ownerText,
  pauseText,
  suppressionScopeText,
  type NotifyAgent,
  type NotifyLogItem,
  type NotifySender,
  type NotifyStatus,
  type NotifySuppression,
} from './email-notify-api'

export const EMAIL_NOTIFICATIONS_MANIFEST: SettingEntry[] = [
  { id: 'email-notifications.senders', section: 'email-notifications', label: 'Notification Senders', description: 'The addresses K2 sends resets, codes, App email and agent notices from', keywords: ['sender', 'noreply', 'system sender', 'relay', 'paused', 'resume', 'bounce'] },
  { id: 'email-notifications.agents', section: 'email-notifications', label: 'Agent Email Notices', description: 'Which workspaces may email the people on this server', keywords: ['agent', 'notice', 'notify', 'workspace', 'email'] },
  { id: 'email-notifications.suppressions', section: 'email-notifications', label: 'Suppressed Addresses', description: 'Addresses K2 stopped sending to after a bounce or a complaint', keywords: ['suppression', 'bounce', 'complaint', 'spam', 'unsubscribe'] },
]

const muted = 'text-[var(--color-text-muted)]'

function Title({ children }: { children: React.ReactNode }): React.JSX.Element {
  return <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">{children}</h3>
}

function fmt(unix: number | null | undefined): string {
  return typeof unix === 'number' && unix > 0 ? new Date(unix * 1000).toLocaleString() : '—'
}

function Chip({ text, tone }: { text: string; tone: 'ok' | 'warn' | 'error' | 'muted' }): React.JSX.Element {
  const color = { ok: 'var(--color-status-ok)', warn: 'var(--color-status-warn)', error: 'var(--color-status-error-soft)', muted: 'var(--color-text-muted)' }[tone]
  return (
    <span className="text-[10px] px-1.5 py-0.5 border" style={{ color, borderColor: color }}>
      {text}
    </span>
  )
}

function SenderRow({ api, s, onResumed }: { api: EmailNotifyApi; s: NotifySender; onResumed: () => void }): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [askPassword, setAskPassword] = useState(false)
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const resume = async (pw?: string): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await api.resume(s, pw)
      setAskPassword(false)
      setPassword('')
      onResumed()
    } catch (e) {
      const msg = notifyErrorMessage(e)
      if (msg.includes('password')) setAskPassword(true)
      setError(msg)
    } finally {
      setBusy(false)
    }
  }
  const tone = s.state === 'ready' ? 'ok' : s.state === 'retired' || s.state === 'off' ? 'muted' : 'warn'
  return (
    <div className="py-2 border-b border-[var(--color-border)]" data-testid={`notify-sender-${s.id}`}>
      <div className="flex items-center gap-2 text-xs">
        <span className="font-medium">{s.address}</span>
        <Chip text={s.state} tone={tone} />
        {s.paused && <Chip text="paused" tone="error" />}
        <span className={muted}>{ownerText(s)}</span>
        <span className={`${muted} ml-auto`}>{s.path}</span>
      </div>
      {s.envelope && <div className={`text-[11px] ${muted}`}>sends through {s.envelope}</div>}
      {s.paused && (
        <div className="mt-1 text-[11px]">
          <span>Paused {fmt(s.paused.pausedAt)}: {pauseText(s.paused)}. Its sends answer email_paused until you resume it.</span>
          <div className="flex items-center gap-2 mt-1">
            {askPassword && (
              <input
                type="password"
                aria-label="Vault password"
                className="text-xs px-2 py-1 bg-transparent border border-[var(--color-border)]"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="Vault password"
              />
            )}
            <button
              type="button"
              className="text-xs px-2 py-1 border border-[var(--color-border)]"
              disabled={busy || (askPassword && !password)}
              onClick={() => void resume(askPassword ? password : undefined)}
            >
              {busy ? 'Confirming…' : 'Resume…'}
            </button>
          </div>
        </div>
      )}
      {error && <div className="text-[11px] text-[var(--color-status-error-soft)] mt-1">{error}</div>}
    </div>
  )
}

export function EmailNotificationsSection({ scope }: { scope: ServerScope }): React.JSX.Element {
  const api = useMemo(() => emailNotifyApi(scope), [scope])
  const [status, setStatus] = useState<NotifyStatus | null>(null)
  const [agents, setAgents] = useState<NotifyAgent[]>([])
  const [sups, setSups] = useState<NotifySuppression[]>([])
  const [log, setLog] = useState<NotifyLogItem[]>([])
  const [search, setSearch] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)

  const load = useCallback(async (): Promise<void> => {
    try {
      const [st, ag, su, lg] = await Promise.all([
        api.status(),
        api.agents(),
        api.suppressions(),
        api.log(30),
      ])
      setStatus(st)
      setAgents(ag)
      setSups(su)
      setLog(lg)
      setError(null)
    } catch (e) {
      setError(notifyErrorMessage(e))
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const find = async (): Promise<void> => {
    try {
      setSups(await api.suppressions(search))
    } catch (e) {
      setError(notifyErrorMessage(e))
    }
  }

  const unsuppress = async (): Promise<void> => {
    if (!search.trim()) return
    try {
      const r = await api.removeSuppression(search.trim())
      setNote(`Removed ${r.removed} entr${r.removed === 1 ? 'y' : 'ies'}: mail may go to ${search.trim()} again.`)
      await find()
    } catch (e) {
      setError(notifyErrorMessage(e))
    }
  }

  const turnOff = async (ws: string): Promise<void> => {
    try {
      await api.agentOff(ws)
      setAgents(await api.agents())
    } catch (e) {
      setError(notifyErrorMessage(e))
    }
  }

  if (error && !status) {
    return (
      <div className="text-xs" data-testid="email-notifications-error">
        <p className="mb-2">Email Notifications couldn't load: {error}</p>
        <p className={muted}>The owner and Admins see this page. From a terminal: k2 notifications email --help.</p>
      </div>
    )
  }
  if (!status) return <div className={`text-xs ${muted}`}>Loading…</div>

  const others = status.senders.filter((s) => s.id !== status.systemSender?.id)
  return (
    <div className="max-w-3xl text-xs space-y-6" data-testid="email-notifications">
      <div>
        <Title>Email Notifications</Title>
        <div className="flex flex-wrap gap-3">
          <span>Switch: <b>{status.boxSwitch}</b></span>
          <span>Mail path: <b>{status.hostedMail ? 'hosted mail on this server' : 'an SMTP relay'}</b></span>
          {status.airgap && <Chip text="air-gap on: nothing leaves" tone="warn" />}
          <span>Log kept {status.settings.logRetentionDays} days</span>
        </div>
        {error && <p className="text-[var(--color-status-error-soft)] mt-2">{error}</p>}
        {note && <p className="mt-2">{note}</p>}
      </div>

      <div>
        <Title>System sender</Title>
        {status.systemSender ? (
          <SenderRow api={api} s={status.systemSender} onResumed={() => void load()} />
        ) : (
          <p className={muted}>None. K2's own resets and codes can't be emailed until you set one: k2 notifications email system-sender set &lt;sender&gt;.</p>
        )}
        <p className={`${muted} mt-1`}>The system sender is never paused: it carries every password reset. A bad bounce rate sends you a warning instead.</p>
      </div>

      <div>
        <Title>Senders</Title>
        {others.length === 0 ? (
          <p className={muted}>No other senders. Add one: k2 notifications email sender add noreply@&lt;your domain&gt;.</p>
        ) : (
          others.map((s) => <SenderRow key={s.id} api={api} s={s} onResumed={() => void load()} />)
        )}
      </div>

      <div>
        <Title>Agent email notices</Title>
        {agents.length === 0 ? (
          <p className={muted}>No workspace may email people. Turn one on: k2 notifications email agent on &lt;workspace&gt; --sender &lt;address&gt;.</p>
        ) : (
          agents.map((a) => (
            <div key={a.workspace} className="flex items-center gap-2 py-1 border-b border-[var(--color-border)]">
              <span className="font-medium">{a.workspace}</span>
              <Chip text={a.on ? 'on' : 'off'} tone={a.on ? 'ok' : 'muted'} />
              <span className={muted}>{a.sender ?? '(sender removed)'}</span>
              {a.offReason && <span className={muted}>({a.offReason})</span>}
              {a.on && (
                <button type="button" className="ml-auto text-xs px-2 py-1 border border-[var(--color-border)]" onClick={() => void turnOff(a.workspace)}>
                  Turn off
                </button>
              )}
            </div>
          ))
        )}
      </div>

      <div>
        <Title>Suppressed addresses</Title>
        <p className={`${muted} mb-2`}>Kept hashed. A hard bounce stops every sender; a spam complaint stops that sender. Search by address to remove one.</p>
        <div className="flex gap-2 mb-2">
          <input
            aria-label="Address"
            className="text-xs px-2 py-1 bg-transparent border border-[var(--color-border)] flex-1"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="someone@example.com"
          />
          <button type="button" className="text-xs px-2 py-1 border border-[var(--color-border)]" onClick={() => void find()}>
            Find
          </button>
          <button type="button" className="text-xs px-2 py-1 border border-[var(--color-border)]" disabled={!search.trim()} onClick={() => void unsuppress()}>
            Remove
          </button>
        </div>
        {sups.length === 0 ? (
          <p className={muted}>Nothing suppressed{search.trim() ? ' for that address' : ''}.</p>
        ) : (
          sups.map((x) => (
            <div key={`${x.addressSha256}-${x.scope}-${x.scopeRef ?? ''}`} className="flex gap-2 py-1 border-b border-[var(--color-border)]">
              <span>@{x.domain}</span>
              <span>{x.reason.replace('_', ' ')}</span>
              <span className={muted}>{suppressionScopeText(x)}</span>
              <span className={`${muted} ml-auto`}>{fmt(x.createdAt)}</span>
            </div>
          ))
        )}
      </div>

      <div>
        <Title>Recent sends</Title>
        {log.length === 0 ? (
          <p className={muted}>Nothing sent yet.</p>
        ) : (
          log.map((r) => (
            <div key={r.id} className="flex gap-2 py-1 border-b border-[var(--color-border)]">
              <span className={muted}>{fmt(r.createdAt)}</span>
              <span>{r.purpose ?? r.origin}</span>
              <span>{r.status}</span>
              {r.error && <span className="text-[var(--color-status-error-soft)]">{r.error}</span>}
              <span className={`${muted} ml-auto`}>{r.to.length} recipient{r.to.length === 1 ? '' : 's'}</span>
            </div>
          ))
        )}
      </div>
    </div>
  )
}
