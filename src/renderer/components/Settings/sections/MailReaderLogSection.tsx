// Settings → Cores → Email Reader Log (prd-ocmail-admin-in-k2-v1 OA2): who
// opened, listed or changed which mailbox, when, from where, and why when
// they gave a reason. Thin table over GET /cli/mail/reader/log; the daemon
// writes every row before the mail is touched and decides what each viewer
// may see. Never shows message content (the log has none).

import React, { useCallback, useEffect, useState } from 'react'
import { primaryScope } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'
import type { SettingEntry } from '../searchManifest'
import { SettingDropdown } from '../controls/SettingControls'
import {
  READER_LOG_SINCE_CHOICES,
  fetchReaderLog,
  readerActionLabel,
  readerActorLabel,
  readerLogTime,
  type ReaderLogRow,
} from './mail-reader-log-api'

export const MAIL_READER_LOG_MANIFEST: SettingEntry[] = [
  {
    id: 'email-reader-log.log',
    section: 'email-reader-log',
    label: 'Email Reader Log',
    description: 'Every time someone opened, listed or changed a mailbox from the Mail page or an agent tab: who, which inbox, when and why',
    keywords: ['mail', 'inbox', 'mailbox', 'reader', 'audit', 'access log', 'opened', 'who read'],
  },
]

const muted = 'text-[var(--color-text-muted)]'
const field = 'px-2 py-1 type-row border border-[var(--color-border)] bg-[var(--color-bg)] text-[var(--color-text-primary)]'

export function MailReaderLogSection(): React.JSX.Element {
  const hostKey = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))
  const [inbox, setInbox] = useState('')
  const [actor, setActor] = useState('')
  const [since, setSince] = useState('7d')
  const [applied, setApplied] = useState({ inbox: '', actor: '' })
  const [rows, setRows] = useState<ReaderLogRow[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const r = await fetchReaderLog(primaryScope(), { inbox: applied.inbox, actor: applied.actor, since })
      setRows(r)
      setError(null)
    } catch (e) {
      setRows(null)
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
    // hostKey: a host switch reloads this server's log.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [applied, since, hostKey])

  useEffect(() => {
    void load()
  }, [load])

  return (
    <div className="flex flex-col gap-3" data-testid="mail-reader-log">
      <div>
        <h3 className="type-section text-[var(--color-text-muted)] uppercase tracking-wider mb-1">Email Reader Log</h3>
        <p className={`type-hint ${muted}`}>
          Every open, list and change of a mailbox by a person (the Mail page, or an agent tab’s Inbox) is written here before the
          mail is touched. The mailbox’s person isn’t notified. No message content is kept.
        </p>
      </div>
      <form
        className="flex flex-wrap items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          setApplied({ inbox, actor })
        }}
      >
        <input data-testid="reader-log-inbox" className={field} placeholder="Inbox address" value={inbox} onChange={(e) => setInbox(e.target.value)} />
        <input data-testid="reader-log-actor" className={field} placeholder="Who (user:name or owner-token)" value={actor} onChange={(e) => setActor(e.target.value)} />
        <SettingDropdown ariaLabel="Since" value={since} onChange={setSince} options={READER_LOG_SINCE_CHOICES} />
        <button type="submit" data-testid="reader-log-apply" className="px-3 py-1 type-row border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)]">
          Filter
        </button>
        <button type="button" onClick={() => void load()} className="px-3 py-1 type-row border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)]">
          Refresh
        </button>
      </form>
      {error && (
        <p data-testid="reader-log-error" className="type-row text-[var(--color-status-error-text)]">
          {error}
        </p>
      )}
      {loading && !rows && <p className={`type-row ${muted}`}>Loading…</p>}
      {rows && rows.length === 0 && <p className={`type-row ${muted}`}>No mailbox access in this window.</p>}
      {rows && rows.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full type-row text-left border-collapse min-w-[760px]">
            <thead>
              <tr className={`border-b border-[var(--color-border)] ${muted}`}>
                <th className="font-medium py-1.5 pr-3">Time</th>
                <th className="font-medium py-1.5 pr-3">Who</th>
                <th className="font-medium py-1.5 pr-3">What</th>
                <th className="font-medium py-1.5 pr-3">Inbox</th>
                <th className="font-medium py-1.5 pr-3">Outcome</th>
                <th className="font-medium py-1.5 pr-3">Reason</th>
                <th className="font-medium py-1.5 pr-3">From</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.id} data-testid={`reader-log-row-${r.id}`} className="border-b border-[var(--color-border)] align-top">
                  <td className="py-1.5 pr-3 whitespace-nowrap">{readerLogTime(r.at)}</td>
                  <td className="py-1.5 pr-3">{readerActorLabel(r)}</td>
                  <td className="py-1.5 pr-3">{readerActionLabel(r.action)}</td>
                  <td className="py-1.5 pr-3">
                    {r.address}
                    <span className={`type-hint ${muted}`}>
                      {' '}
                      · {r.kind === 'person' ? (r.own ? 'their own' : 'a person’s') : 'agent'} · {r.source}
                    </span>
                  </td>
                  <td
                    className={`py-1.5 pr-3 ${r.outcome === 'ok' ? 'text-[var(--color-status-ok)]' : 'text-[var(--color-status-error-text)]'}`}
                  >
                    {r.outcome === 'ok' ? 'allowed' : 'refused'}
                    {typeof r.status === 'number' ? <span className={`type-hint ${muted}`}> {r.status}</span> : null}
                  </td>
                  <td className="py-1.5 pr-3">{r.reason || <span className={muted}>—</span>}</td>
                  <td className={`py-1.5 pr-3 type-hint ${muted}`}>{r.ipHint || '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}
