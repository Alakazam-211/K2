// Settings → Cores → Email Reader Log (prd-ocmail-admin-in-k2-v1 OA2): who
// opened, listed or changed which mailbox, when, from where, and why when
// they gave a reason. A list over GET /cli/mail/reader/log; the daemon
// writes every row before the mail is touched and decides what each viewer
// may see. Never shows message content (the log has none).
//
// The Settings page anatomy (as Access Audit): a heading and its hint, the
// filters in one row (mailbox, who, how far back), then the reads in one
// card, each a sentence that opens to every detail; the shared load state
// in the card's place while it loads, after a failed read, or when nothing
// matches.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { primaryScope } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'
import { Button, SearchField, Tag } from '@/components/ui'
import type { SettingEntry } from '../searchManifest'
import { SETTING_LIST_ROW_HOVER, SETTING_ROW_HINT, SettingDropdown, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { localWhen, localWhenFull } from '../shared/local-time'
import {
  READER_LOG_DEFAULT_LIMIT,
  READER_LOG_SINCE_CHOICES,
  ReaderLogUnreadableError,
  fetchReaderLog,
  readerActionPhrase,
  readerActorName,
  readerFromText,
  readerMailboxText,
  readerRoleText,
  type ReaderLogRow,
} from './mail-reader-log-api'
import { PRODUCT_NAME } from '@/lib/brand'

export const MAIL_READER_LOG_MANIFEST: SettingEntry[] = [
  {
    id: 'email-reader-log.log',
    section: 'email-reader-log',
    label: 'Email Reader Log',
    description: 'Every time someone opened, listed or changed a mailbox from the Mail page or an agent tab: who, which inbox, when and why',
    keywords: ['mail', 'inbox', 'mailbox', 'reader', 'audit', 'access log', 'opened', 'who read'],
  },
]

const CONTENT_WIDTH = 'max-w-[52rem]'
const EVERYONE = ''
const FULL_ADDRESS = /^[^\s@]+@[^\s@]+\.[^\s@]+$/

type View = { kind: 'loading' } | { kind: 'rows'; rows: ReaderLogRow[] } | { kind: 'error'; detail: string; unreadable: boolean }

function ReadRow({ r, open, onToggle }: { r: ReaderLogRow; open: boolean; onToggle: () => void }): React.JSX.Element {
  const refused = r.outcome !== 'ok'
  const { phrase, agentTab } = readerActionPhrase(r.action)
  const name = readerActorName(r.actor)
  // Listing inboxes isn't "in" one mailbox: the address goes to the hint.
  const listing = r.action === 'reader/inboxes' || r.action === 'inboxes'
  const rowId = `reader-log-details-${r.id}`
  const fields: { label: string; value: React.ReactNode }[] = [
    { label: 'When', value: localWhenFull(r.at * 1000) },
    { label: 'Who', value: r.actorRole && r.actor !== 'owner-token' ? `${name}, ${readerRoleText(r.actorRole)}` : name },
    { label: 'Mailbox', value: `${r.address || '—'} · ${readerMailboxText(r)}` },
    { label: 'Through', value: agentTab ? 'An agent tab’s Inbox' : 'The Mail page' },
    {
      label: 'Outcome',
      value: refused ? `Refused: ${PRODUCT_NAME} didn’t open the mail` : 'Allowed',
    },
    { label: 'Reason given', value: r.reason || 'None given' },
    { label: 'From', value: readerFromText(r.ipHint) },
  ]
  return (
    <li data-testid={`reader-log-row-${r.id}`} data-refused={refused ? 'true' : undefined}>
      <button
        type="button"
        data-k2-focus=""
        aria-expanded={open}
        aria-controls={rowId}
        onClick={onToggle}
        className={`group relative w-full grid grid-cols-[auto_minmax(0,1fr)_auto_auto] items-start gap-x-3 px-4 py-3 text-left no-drag cursor-pointer outline-none ${SETTING_LIST_ROW_HOVER}`}
      >
        <span
          aria-hidden="true"
          className={`mt-1.5 inline-block h-2 w-2 rounded-[var(--radius-badge)] ${refused ? 'bg-[var(--color-status-error)]' : 'bg-[var(--color-status-ok)]'}`}
        />
        <span className="min-w-0">
          <span className="block type-row text-[var(--color-text-primary)] break-words">
            <span className="type-name">{name}</span> {phrase}
            {r.address && !listing ? (
              <>
                {' in '}
                <span className="type-name">{r.address}</span>
              </>
            ) : null}
            {refused && <Tag data-case="as-written" tone="danger" className="ml-2 align-middle">Refused</Tag>}
          </span>
          <span className="block type-hint text-[var(--color-text-secondary)] mt-0.5 break-words">
            {listing && r.address ? `Including ${r.address} · ` : ''}
            {readerMailboxText(r)}
            {agentTab ? ' · From an agent tab' : ''}
            {r.reason ? ` · “${r.reason}”` : ''}
          </span>
        </span>
        <span className="type-hint text-[var(--color-text-secondary)] tabular-nums whitespace-nowrap" title={new Date(r.at * 1000).toUTCString()}>
          {localWhen(r.at * 1000)}
        </span>
        <span aria-hidden="true" className={`type-row text-[var(--color-text-muted)] group-hover:text-[var(--color-text-primary)] transition-transform ${open ? 'rotate-90' : ''}`}>
          ›
        </span>
      </button>
      {open && (
        <dl id={rowId} className="grid grid-cols-[9rem_minmax(0,1fr)] gap-x-4 gap-y-1 px-4 pb-3 pl-9" data-testid={`reader-log-details-${r.id}`}>
          {fields.map((f) => (
            <React.Fragment key={f.label}>
              <dt className="type-hint text-[var(--color-text-secondary)]">{f.label}</dt>
              <dd className="type-hint text-[var(--color-text-primary)] break-words">{f.value}</dd>
            </React.Fragment>
          ))}
        </dl>
      )}
    </li>
  )
}

export function MailReaderLogSection(): React.JSX.Element {
  const hostKey = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))
  const [mailbox, setMailbox] = useState('')
  const [actor, setActor] = useState(EVERYONE)
  const [since, setSince] = useState('7d')
  /** A full address typed in the mailbox field also narrows the server's
   *  read, so older reads of that mailbox come back too. */
  const [inbox, setInbox] = useState('')
  const [view, setView] = useState<View>({ kind: 'loading' })
  const [busy, setBusy] = useState(false)
  const [openRow, setOpenRow] = useState<number | null>(null)
  /** Everyone seen in the log so far, for the Who menu. */
  const [seen, setSeen] = useState<string[]>([])

  useEffect(() => {
    const q = mailbox.trim().toLowerCase()
    const next = FULL_ADDRESS.test(q) ? q : ''
    const t = setTimeout(() => setInbox(next), 400)
    return () => clearTimeout(t)
  }, [mailbox])

  const load = useCallback(async () => {
    setBusy(true)
    try {
      const rows = await fetchReaderLog(primaryScope(), { inbox, actor, since })
      setView({ kind: 'rows', rows })
      setSeen((prev) => {
        const next = new Set(prev)
        for (const r of rows) next.add(r.actor)
        return next.size === prev.length ? prev : [...next]
      })
    } catch (e) {
      setView({
        kind: 'error',
        detail: e instanceof Error ? e.message : String(e),
        unreadable: e instanceof ReaderLogUnreadableError,
      })
    } finally {
      setBusy(false)
    }
    // hostKey: a host switch reloads this server's log.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inbox, actor, since, hostKey])

  useEffect(() => {
    void load()
  }, [load])

  const q = mailbox.trim().toLowerCase()
  const all = view.kind === 'rows' ? view.rows : []
  const rows = useMemo(() => (q ? all.filter((r) => r.address.toLowerCase().includes(q)) : all), [all, q])
  const narrowed = q !== '' || actor !== EVERYONE
  const sinceText = since ? (READER_LOG_SINCE_CHOICES.find((c) => c.value === since)?.label.toLowerCase() ?? since) : 'everything kept'
  const windowText = since ? `in the ${sinceText}` : 'in everything kept'
  const who = actor ? readerActorName(actor) : ''
  const clear = (): void => {
    setMailbox('')
    setInbox('')
    setActor(EVERYONE)
    setOpenRow(null)
  }

  const whoOptions = useMemo(() => {
    const names = [...new Set([...seen, ...(actor ? [actor] : [])])].sort((a, b) => readerActorName(a).localeCompare(readerActorName(b)))
    return [{ value: EVERYONE, label: 'Everyone' }, ...names.map((a) => ({ value: a, label: readerActorName(a) }))]
  }, [seen, actor])

  const tryAgain = (
    <Button onClick={() => void load()} disabled={busy}>
      {busy ? 'Trying…' : 'Try again'}
    </Button>
  )

  const state =
    view.kind === 'loading' ? (
      <SettingsLoadState kind="loading" title="Loading mailbox reads…" testId="reader-log-loading">
        Who opened which mailbox, newest first.
      </SettingsLoadState>
    ) : view.kind === 'error' ? (
      <SettingsLoadState
        kind="error"
        title="The reader log couldn’t load"
        detail={view.unreadable ? null : view.detail}
        action={tryAgain}
        testId="reader-log-error-state"
      >
        <span data-testid="reader-log-error">
          {view.unreadable
            ? `This server sent an answer ${PRODUCT_NAME} couldn’t read, so there’s nothing to show yet. Try again; if it keeps happening, update ${PRODUCT_NAME} on that server.`
            : `${PRODUCT_NAME} couldn’t read this server’s mailbox log, so there’s nothing to show yet. Nothing in it is lost; try again in a moment.`}
        </span>
      </SettingsLoadState>
    ) : all.length === 0 && !narrowed ? (
      <SettingsLoadState
        kind="empty"
        title={since ? `No mailbox reads ${windowText}` : 'No mailbox reads yet'}
        action={since !== '' ? <Button onClick={() => setSince('')}>Show everything kept</Button> : undefined}
        testId="reader-log-empty"
      >
        Each time someone opens, lists or changes a mailbox, it shows here.
      </SettingsLoadState>
    ) : undefined

  return (
    <div data-settings-id="email-reader-log" data-testid="mail-reader-log" className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`}>
      <header className="min-w-0" data-settings-id="email-reader-log.log">
        <h2 className="type-title text-[var(--color-text-primary)]">Email Reader Log</h2>
        <p className={SETTING_ROW_HINT}>
          Each time someone opens, lists or changes a mailbox from the Mail page or an agent tab, {PRODUCT_NAME} records it here before the
          mail is opened. The mailbox’s owner isn’t told, and no message content is kept.
        </p>
      </header>

      <div role="search" aria-label="Filter the log" className="flex flex-wrap items-center gap-2" data-testid="reader-log-filters">
        <SearchField
          data-testid="reader-log-inbox"
          aria-label="Filter by mailbox"
          className="w-60"
          placeholder="Filter by mailbox"
          value={mailbox}
          onChange={(e) => {
            setMailbox(e.target.value)
            setOpenRow(null)
          }}
        />
        <span className="flex items-center gap-2">
          <span className="type-row text-[var(--color-text-secondary)]">Who</span>
          <SettingDropdown ariaLabel="Who" value={actor} onChange={(v) => setActor(v)} options={whoOptions} menuAlign="left" triggerData={{ 'data-testid': 'reader-log-actor' }} />
        </span>
        <span className="flex items-center gap-2">
          <span className="type-row text-[var(--color-text-secondary)]">When</span>
          <SettingDropdown ariaLabel="When" value={since} onChange={(v) => setSince(v)} options={READER_LOG_SINCE_CHOICES} menuAlign="left" triggerData={{ 'data-testid': 'reader-log-since' }} />
        </span>
        <Button className="ml-auto" onClick={() => void load()} disabled={busy} data-testid="reader-log-refresh">
          {busy && view.kind === 'rows' ? 'Refreshing…' : 'Refresh'}
        </Button>
      </div>

      <SettingsGroup
        card
        title="Mailbox reads"
        hint={
          narrowed
            ? rows.length === 0
              ? 'Nothing matches these filters.'
              : `${rows.length === 1 ? 'One read' : `${rows.length} reads`}${q ? ` of mailboxes matching “${mailbox.trim()}”` : ''}${who ? ` by ${who}` : ''}, newest first.`
            : 'Newest first. Open a read for every detail.'
        }
        state={state}
        testId="reader-log-table"
      >
        {rows.length === 0 ? (
          <div className="px-4 py-6 flex items-center justify-between gap-6" data-testid="reader-log-no-match">
            <div className="min-w-0">
              <p className="type-row text-[var(--color-text-primary)]">Nothing matches</p>
              <p className={SETTING_ROW_HINT}>
                No reads {q ? `of a mailbox matching “${mailbox.trim()}” ` : ''}
                {who ? `by ${who} ` : ''}
                {windowText}.
              </p>
            </div>
            <Button onClick={clear} data-testid="reader-log-clear">
              Show everyone
            </Button>
          </div>
        ) : (
          <ul className="divide-y divide-[var(--color-border)]" data-testid="reader-log-rows">
            {rows.map((r) => (
              <ReadRow key={r.id} r={r} open={openRow === r.id} onToggle={() => setOpenRow((k) => (k === r.id ? null : r.id))} />
            ))}
          </ul>
        )}
        <div className="px-4 py-2.5 flex items-center justify-between gap-4">
          <span className="type-hint text-[var(--color-text-secondary)]" data-testid="reader-log-count">
            {narrowed ? `Showing ${rows.length} of ` : ''}
            {all.length === 1 ? (narrowed ? 'one read' : 'One read') : `${all.length} reads`} {windowText}
            {all.length >= READER_LOG_DEFAULT_LIMIT ? ` (${PRODUCT_NAME} shows the newest ${READER_LOG_DEFAULT_LIMIT})` : ''}
          </span>
          {narrowed && rows.length > 0 && (
            <Button variant="quiet" onClick={clear}>
              Show everyone
            </Button>
          )}
        </div>
      </SettingsGroup>
    </div>
  )
}
