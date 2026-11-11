// Settings → Cores → Email Reader Log (prd-ocmail-admin-in-k2-v1 OA2): who
// opened, listed or changed which mailbox, when, from where, and why when
// they gave a reason. Thin table over GET /cli/mail/reader/log; the daemon
// writes every row before the mail is touched and decides what each viewer
// may see. Never shows message content (the log has none).
//
// The Settings page anatomy (as Usage): a heading and its hint, the filters
// as a toolbar, then the log in one tile (a cut card in Terminal) or the
// shared load state in its place while it loads, after a failed read, or
// when nothing matches.

import React, { useCallback, useEffect, useState } from 'react'
import { primaryScope } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'
import { Button, Input, Surface, Tag } from '@/components/ui'
import type { SettingEntry } from '../searchManifest'
import { SETTING_ROW_HINT, SettingDropdown, SettingsLoadState } from '../controls/SettingControls'
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

/** One log row's columns, shared by the head and every row so they line up. */
const ROW_GRID =
  'grid grid-cols-[minmax(9rem,11rem)_minmax(7rem,9rem)_minmax(10rem,1.2fr)_minmax(12rem,1.6fr)_6.5rem_minmax(8rem,1fr)_minmax(6rem,8rem)] items-start gap-x-4 px-4'

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

  const filtered = applied.inbox.trim() !== '' || applied.actor.trim() !== ''
  const sinceLabel = since ? `the ${READER_LOG_SINCE_CHOICES.find((c) => c.value === since)?.label.toLowerCase() ?? since}` : 'the log'

  return (
    <div className="flex flex-col gap-4 max-w-[96rem]" data-testid="mail-reader-log">
      <div>
        <div className="flex items-center justify-between gap-4 min-h-[1.5rem]">
          <h3 className="type-heading text-[var(--color-text-primary)]">Email Reader Log</h3>
          <Button variant="quiet" data-testid="reader-log-refresh" disabled={loading} onClick={() => void load()}>
            {loading && rows ? 'Refreshing…' : 'Refresh'}
          </Button>
        </div>
        <p className={SETTING_ROW_HINT}>
          Every open, list and change of a mailbox by a person (the Mail page, or an agent tab’s Inbox) is written here before the
          mail is touched. The mailbox’s person isn’t notified. No message content is kept.
        </p>
      </div>
      <form
        role="search"
        aria-label="Filter the log"
        className="flex flex-wrap items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          setApplied({ inbox, actor })
        }}
      >
        <Input
          data-testid="reader-log-inbox"
          aria-label="Inbox address"
          className="w-56"
          placeholder="Inbox address"
          value={inbox}
          onChange={(e) => setInbox(e.target.value)}
        />
        <Input
          data-testid="reader-log-actor"
          aria-label="Who"
          className="w-56"
          placeholder="Who (user:name or owner-token)"
          value={actor}
          onChange={(e) => setActor(e.target.value)}
        />
        <SettingDropdown ariaLabel="Since" value={since} onChange={setSince} options={READER_LOG_SINCE_CHOICES} />
        <Button type="submit" variant="default" data-testid="reader-log-apply">
          Filter
        </Button>
        {filtered && (
          <Button
            variant="quiet"
            data-testid="reader-log-clear"
            onClick={() => {
              setInbox('')
              setActor('')
              setApplied({ inbox: '', actor: '' })
            }}
          >
            Show everyone
          </Button>
        )}
      </form>
      {error ? (
        <SettingsLoadState
          kind="error"
          title="The reader log couldn’t load"
          action={
            <Button className="flex-shrink-0" onClick={() => void load()}>
              Try again
            </Button>
          }
          testId="reader-log-error-state"
        >
          <span data-testid="reader-log-error">{error}</span>
        </SettingsLoadState>
      ) : !rows ? (
        <SettingsLoadState kind="loading" title="Loading the log…" testId="reader-log-loading">
          Who opened which mailbox, newest first.
        </SettingsLoadState>
      ) : rows.length === 0 ? (
        <SettingsLoadState kind="empty" title="No mailbox access in this window" testId="reader-log-empty">
          {filtered ? 'Nothing matches these filters' : 'No one opened, listed or changed a mailbox'} in {sinceLabel}.
        </SettingsLoadState>
      ) : (
        <Surface chamfer material="tile" className="overflow-hidden" data-testid="reader-log-table">
          <div className="overflow-x-auto">
            <div role="table" aria-label="Email Reader Log" className="min-w-[60rem]">
              <div role="row" className={`${ROW_GRID} py-2 border-b border-[var(--color-border)] type-hint text-[var(--color-text-muted)]`}>
                {['Time', 'Who', 'What', 'Inbox', 'Outcome', 'Reason', 'From'].map((c) => (
                  <span key={c} role="columnheader">
                    {c}
                  </span>
                ))}
              </div>
              <div role="rowgroup" className="divide-y divide-[var(--color-border)]">
                {rows.map((r) => (
                  <div key={r.id} role="row" data-testid={`reader-log-row-${r.id}`} className={`${ROW_GRID} py-2.5 type-row`}>
                    <span role="cell" className="tabular-nums text-[var(--color-text-secondary)]">
                      {readerLogTime(r.at)}
                    </span>
                    <span role="cell" className="text-[var(--color-text-primary)] break-words">
                      {readerActorLabel(r)}
                    </span>
                    <span role="cell" className="text-[var(--color-text-primary)]">
                      {readerActionLabel(r.action)}
                    </span>
                    <span role="cell" className="min-w-0">
                      <span className="block truncate text-[var(--color-text-primary)]" title={r.address}>
                        {r.address}
                      </span>
                      <span className="block type-hint text-[var(--color-text-muted)]">
                        {r.kind === 'person' ? (r.own ? 'their own' : 'a person’s') : 'agent'} · {r.source}
                      </span>
                    </span>
                    <span role="cell">
                      <Tag
                        tone={r.outcome === 'ok' ? 'ok' : 'danger'}
                        data-case="as-written"
                        title={typeof r.status === 'number' ? `HTTP ${r.status}` : undefined}
                      >
                        {r.outcome === 'ok' ? 'allowed' : 'refused'}
                      </Tag>
                    </span>
                    <span role="cell" className={r.reason ? 'text-[var(--color-text-primary)] break-words' : 'text-[var(--color-text-muted)]'}>
                      {r.reason || '—'}
                    </span>
                    <span role="cell" className="type-hint text-[var(--color-text-muted)] break-words">
                      {r.ipHint || '—'}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </Surface>
      )}
    </div>
  )
}
