// The whole reader: inboxes → folders → messages → the conversation, plus a
// composer. Layout follows OCmail's page (sidebar, list, viewer); state is
// local to this component, and every server call goes through the host's
// MailReaderSource.
//
// Opening anything only reads (`inboxes`, `folders`, `list`, `thread`). The
// write methods are called from buttons only: Mark read/unread and Star
// (`flag`), Move, Archive, Delete (asks first), Reply / New message → Send
// (asks first) or Save draft. Refreshes on window focus and on the button;
// nothing is pushed.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { folderLabel, folderOrder, folderParam } from '../format'
import { t } from '../i18n'
import type { MailReaderSource } from '../source'
import type {
  MailWriteResult,
  ReaderAttachment,
  ReaderAttachmentFile,
  ReaderEmail,
  ReaderFolder,
  ReaderInbox,
  ReaderMessage,
  ReaderViewer,
  ThreadGroup,
} from '../types'
import { Composer, type ComposeMode, type ComposeValues } from './Composer'
import { EmailList } from './EmailList'
import type { MessageActions, RenderHtml } from './EmailViewer'
import { FolderList } from './FolderList'
import { InboxList } from './InboxList'
import { ThreadConversationView } from './ThreadConversationView'

export interface ConfirmOptions {
  title: string
  message: string
  confirmLabel: string
  destructive?: boolean
}

export interface MailReaderProps {
  source: MailReaderSource
  renderHtml: RenderHtml
  /** Ask before Send and Delete (default: window.confirm). */
  confirm?: (opts: ConfirmOptions) => Promise<boolean>
  /** Save a downloaded attachment (default: an anchor download). */
  saveAttachment?: (file: ReaderAttachmentFile) => Promise<unknown> | void
  /** Messages per page (max 100, the daemon's cap). */
  pageSize?: number
  /** Shown above the reader (K2: the "doesn't mark read, every access is logged" bar). */
  banner?: React.ReactNode
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

async function defaultConfirm(o: ConfirmOptions): Promise<boolean> {
  return typeof window !== 'undefined' && typeof window.confirm === 'function' ? window.confirm(`${o.title}\n\n${o.message}`) : false
}

function defaultSave(file: ReaderAttachmentFile): void {
  const url = URL.createObjectURL(file.blob)
  try {
    const a = document.createElement('a')
    a.href = url
    a.download = file.filename || 'attachment'
    a.rel = 'noopener'
    document.body.appendChild(a)
    a.click()
    a.remove()
  } finally {
    setTimeout(() => URL.revokeObjectURL(url), 1000)
  }
}

/** A write's answer in words (send may wait for approval). */
export function describeWriteResult(r: MailWriteResult, fallback: string): string {
  const status = typeof r.status === 'string' ? r.status : ''
  if (status === 'pending_approval') return t('sent_pending')
  if (status === 'failed') return t('error_prefix', { message: typeof r.hint === 'string' ? r.hint : 'the send failed' })
  return fallback
}

export function MailReader({ source, renderHtml, confirm = defaultConfirm, saveAttachment = defaultSave, pageSize = 50, banner }: MailReaderProps): React.JSX.Element {
  const limit = Math.max(1, Math.min(100, pageSize))
  const [viewer, setViewer] = useState<ReaderViewer | null>(null)
  const [inboxes, setInboxes] = useState<ReaderInbox[]>([])
  const [inboxError, setInboxError] = useState<string | null>(null)
  const [inbox, setInbox] = useState<string | null>(null)
  const [folders, setFolders] = useState<ReaderFolder[]>([])
  const [folder, setFolder] = useState('inbox')
  const [query, setQuery] = useState('')
  const [appliedQuery, setAppliedQuery] = useState('')
  const [unreadOnly, setUnreadOnly] = useState(false)
  const [emails, setEmails] = useState<ReaderEmail[]>([])
  const [nextOffset, setNextOffset] = useState<number | null>(null)
  const [listLoading, setListLoading] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)
  const [listError, setListError] = useState<string | null>(null)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [thread, setThread] = useState<ReaderMessage[] | null>(null)
  const [threadLoading, setThreadLoading] = useState(false)
  const [threadError, setThreadError] = useState<string | null>(null)
  const [starred, setStarred] = useState<Set<string>>(() => new Set())
  const [compose, setCompose] = useState<ComposeMode | null>(null)
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const listSeq = useRef(0)
  const threadSeq = useRef(0)

  const loadInboxes = useCallback(async () => {
    try {
      const r = await source.inboxes()
      setViewer(r.viewer)
      setInboxes(r.inboxes)
      setInboxError(null)
      setInbox((cur) => {
        if (cur && r.inboxes.some((i) => i.address === cur)) return cur
        const first = r.inboxes.find((i) => i.own) ?? r.inboxes[0]
        return first ? first.address : null
      })
    } catch (e) {
      setInboxError(errText(e))
    }
  }, [source])

  const loadList = useCallback(
    async (opts: { more?: boolean; quiet?: boolean } = {}) => {
      if (!inbox) return
      const seq = ++listSeq.current
      const offset = opts.more ? (nextOffset ?? 0) : 0
      if (opts.more) setLoadingMore(true)
      else if (!opts.quiet) setListLoading(true)
      try {
        const page = await source.list(inbox, {
          folder,
          offset,
          limit,
          query: appliedQuery || undefined,
          unread: unreadOnly || undefined,
        })
        if (seq !== listSeq.current) return
        setEmails((cur) => (opts.more ? [...cur, ...page.items.filter((e) => !cur.some((c) => c.id === e.id))] : page.items))
        setNextOffset(page.nextOffset)
        setListError(null)
      } catch (e) {
        if (seq !== listSeq.current) return
        setListError(errText(e))
        if (!opts.more) setEmails([])
      } finally {
        if (seq === listSeq.current) {
          setListLoading(false)
          setLoadingMore(false)
        }
      }
    },
    [source, inbox, folder, limit, appliedQuery, unreadOnly, nextOffset],
  )

  useEffect(() => {
    void loadInboxes()
  }, [loadInboxes])

  // A new inbox: its folders, back to Inbox, nothing open.
  useEffect(() => {
    setFolders([])
    setFolder('inbox')
    setSelectedId(null)
    setThread(null)
    setCompose(null)
    if (!inbox) return
    let gone = false
    source
      .folders(inbox)
      .then((f) => {
        if (!gone) setFolders(f)
      })
      .catch((e: unknown) => {
        if (!gone) setListError(errText(e))
      })
    return () => {
      gone = true
    }
  }, [source, inbox])

  // The list follows the inbox, folder and filters (not nextOffset).
  useEffect(() => {
    void loadList()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source, inbox, folder, appliedQuery, unreadOnly, limit])

  const refresh = useCallback(() => {
    void loadInboxes()
    void loadList({ quiet: true })
  }, [loadInboxes, loadList])

  const refreshRef = useRef(refresh)
  refreshRef.current = refresh
  useEffect(() => {
    if (typeof window === 'undefined') return
    const onFocus = (): void => refreshRef.current()
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [])

  const openThread = useCallback(
    async (group: ThreadGroup) => {
      const id = group.latestEmail.id
      const seq = ++threadSeq.current
      setSelectedId(id)
      setCompose(null)
      setNotice(null)
      setThreadLoading(true)
      setThreadError(null)
      try {
        // A peek: the daemon never marks these read.
        const messages = await source.thread(id, folder)
        if (seq !== threadSeq.current) return
        setThread(messages.length > 0 ? messages : null)
      } catch (e) {
        if (seq !== threadSeq.current) return
        setThread(null)
        setThreadError(errText(e))
      } finally {
        if (seq === threadSeq.current) setThreadLoading(false)
      }
    },
    [source, folder],
  )

  const patchMessage = (id: string, patch: Partial<ReaderMessage>): void => {
    setThread((cur) => (cur ? cur.map((m) => (m.id === id ? { ...m, ...patch } : m)) : cur))
    setEmails((cur) => cur.map((e) => (e.id === id ? { ...e, ...patch } : e)))
  }

  const dropMessage = (id: string): void => {
    setEmails((cur) => cur.filter((e) => e.id !== id))
    setThread((cur) => {
      const left = cur ? cur.filter((m) => m.id !== id) : cur
      return left && left.length > 0 ? left : null
    })
    if (selectedId === id) setSelectedId(null)
  }

  const run = async (what: () => Promise<MailWriteResult>, done: (r: MailWriteResult) => string): Promise<boolean> => {
    setBusy(true)
    setNotice(null)
    try {
      const r = await what()
      setNotice(done(r ?? {}))
      return true
    } catch (e) {
      setNotice(t('error_prefix', { message: errText(e) }))
      return false
    } finally {
      setBusy(false)
    }
  }

  const moveTargets = useMemo(
    () =>
      folderOrder(folders)
        .filter((f) => folderParam(f).toLowerCase() !== folder.toLowerCase())
        .map((f) => ({ value: folderParam(f), label: folderLabel(f) })),
    [folders, folder],
  )

  const currentInbox = inboxes.find((i) => i.address === inbox) ?? null

  const actionsFor = (m: ReaderMessage): MessageActions => ({
    busy,
    starred: starred.has(m.id),
    moveTargets,
    onReply: (msg) =>
      setCompose({ kind: 'reply', inbox: msg.inbox, id: msg.id, subject: msg.subject || '', to: msg.from?.address || '' }),
    onToggleRead: (msg) =>
      void run(
        () => source.flag(msg.id, { read: msg.unread }),
        () => {
          patchMessage(msg.id, { unread: !msg.unread })
          return t('done')
        },
      ),
    onToggleStar: (msg) => {
      const on = !starred.has(msg.id)
      void run(
        () => source.flag(msg.id, { flagged: on }),
        () => {
          setStarred((s) => {
            const n = new Set(s)
            if (on) n.add(msg.id)
            else n.delete(msg.id)
            return n
          })
          return t('done')
        },
      )
    },
    onMove: (msg, to) =>
      void run(
        () => source.move(msg.id, to),
        () => {
          dropMessage(msg.id)
          return t('done')
        },
      ),
    onArchive: (msg) =>
      void run(
        () => source.archive(msg.id),
        () => {
          dropMessage(msg.id)
          return t('done')
        },
      ),
    onDelete: (msg) =>
      void (async () => {
        const ok = await confirm({
          title: t('confirm_delete_title'),
          message: t('confirm_delete_message', { inbox: msg.inbox }),
          confirmLabel: t('delete'),
          destructive: true,
        })
        if (!ok) return
        await run(
          () => source.delete(msg.id),
          () => {
            dropMessage(msg.id)
            return t('done')
          },
        )
      })(),
    onDownload: (msg: ReaderMessage, att: ReaderAttachment) =>
      void (async () => {
        setBusy(true)
        try {
          const file = await source.attachment(msg.id, att.index)
          await saveAttachment(file)
        } catch (e) {
          setNotice(t('error_prefix', { message: errText(e) }))
        } finally {
          setBusy(false)
        }
      })(),
  })

  const send = async (mode: ComposeMode, v: ComposeValues): Promise<void> => {
    const to = mode.kind === 'reply' ? mode.to : v.to.join(', ')
    const ok = await confirm({
      title: t('confirm_send_title'),
      message: t('confirm_send_message', { inbox: mode.inbox, to }),
      confirmLabel: t('send'),
    })
    if (!ok) return
    const sent = await run(
      () =>
        mode.kind === 'reply'
          ? source.reply({ id: mode.id, body: v.body })
          : source.send({ inbox: mode.inbox, to: v.to, cc: v.cc, subject: v.subject, body: v.body }),
      (r) => describeWriteResult(r, t('sent_ok')),
    )
    if (sent) setCompose(null)
  }

  const saveDraft = async (mode: ComposeMode, v: ComposeValues): Promise<void> => {
    const saved = await run(
      () =>
        mode.kind === 'reply'
          ? source.draft({ id: mode.id, body: v.body })
          : source.draft({ inbox: mode.inbox, to: v.to, cc: v.cc, subject: v.subject, body: v.body }),
      () => t('draft_saved'),
    )
    if (saved) setCompose(null)
  }

  return (
    <div data-mr="reader" data-testid="mr-reader" className="flex flex-col h-full min-h-0 text-[var(--color-text-primary)]">
      {banner}
      <div className="flex flex-1 min-h-0">
        <aside data-mr="sidebar" className="w-60 flex-shrink-0 border-r border-[var(--color-border)] overflow-y-auto">
          {inboxError && (
            <p data-testid="mr-inbox-error" className="px-3 py-2 type-row text-[var(--color-status-error-text)]">
              {inboxError}
            </p>
          )}
          <InboxList viewer={viewer} inboxes={inboxes} selected={inbox} onSelect={setInbox} />
          {inbox && folders.length > 0 && (
            <div className="border-t border-[var(--color-border)]">
              <FolderList folders={folders} selected={folder} onSelect={setFolder} />
            </div>
          )}
        </aside>
        <section data-mr="list-pane" className="w-80 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0">
          <form
            className="flex items-center gap-1.5 px-3 py-2 border-b border-[var(--color-border)]"
            onSubmit={(e) => {
              e.preventDefault()
              setAppliedQuery(query.trim())
            }}
          >
            <input
              data-testid="mr-search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t('search_placeholder')}
              className="flex-1 min-w-0 px-2 py-0.5 type-row border border-[var(--color-border)] bg-[var(--color-bg)]"
            />
            <label className="flex items-center gap-1 type-hint text-[var(--color-text-muted)]">
              <input type="checkbox" data-testid="mr-unread-only" checked={unreadOnly} onChange={(e) => setUnreadOnly(e.target.checked)} />
              {t('unread_only')}
            </label>
            <button
              type="button"
              data-testid="mr-refresh"
              onClick={refresh}
              title={t('refresh')}
              className="px-1.5 py-0.5 type-row border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)]"
            >
              ↻
            </button>
          </form>
          {inbox && (
            <div className="px-3 py-1.5 border-b border-[var(--color-border)] flex items-center gap-2">
              <span className="truncate type-hint text-[var(--color-text-muted)]">{currentInbox?.displayName || inbox}</span>
              <button
                type="button"
                data-testid="mr-new-message"
                disabled={busy}
                onClick={() => {
                  setNotice(null)
                  setCompose({ kind: 'new', inbox })
                }}
                className="ml-auto px-2 py-0.5 type-row border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)]"
              >
                {t('new_message')}
              </button>
            </div>
          )}
          {inbox ? (
            <EmailList
              emails={emails}
              loading={listLoading}
              loadingMore={loadingMore}
              hasMore={nextOffset !== null}
              error={listError}
              selectedId={selectedId}
              onSelect={(g) => void openThread(g)}
              onLoadMore={() => void loadList({ more: true })}
            />
          ) : (
            <p className="px-4 py-6 type-row text-[var(--color-text-muted)]">{t('pick_inbox')}</p>
          )}
        </section>
        <section data-mr="message-pane" className="flex-1 min-w-0 overflow-y-auto">
          {notice && (
            <p data-testid="mr-notice" className="mx-4 mt-3 px-2 py-1 type-row border border-[var(--color-border)] bg-[var(--color-bg-elevated)]">
              {notice}
            </p>
          )}
          {compose && (
            <Composer
              key={compose.kind === 'reply' ? `r:${compose.id}` : `n:${compose.inbox}`}
              mode={compose}
              busy={busy}
              onSend={(v) => void send(compose, v)}
              onSaveDraft={(v) => void saveDraft(compose, v)}
              onCancel={() => setCompose(null)}
            />
          )}
          {threadLoading && <p className="px-4 py-6 type-row text-[var(--color-text-muted)]">{t('loading')}</p>}
          {threadError && (
            <p data-testid="mr-thread-error" className="px-4 py-3 type-row text-[var(--color-status-error-text)]">
              {threadError}
            </p>
          )}
          {!threadLoading && !threadError && thread && (
            <ThreadConversationView messages={thread} renderHtml={renderHtml} actionsFor={actionsFor} />
          )}
          {!threadLoading && !threadError && !thread && !compose && inbox && (
            <p className="px-4 py-6 type-row text-[var(--color-text-muted)]">{t('pick_message')}</p>
          )}
        </section>
      </div>
    </div>
  )
}

