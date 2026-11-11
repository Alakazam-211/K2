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
//
// Layout (./panes): three resizable columns from 840px of the reader's own
// width; below that one column at a time (mailboxes → list → conversation)
// with Back. The controls and icons are the host's (`ui`, see ../ui).

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
import { IconButton, MailIcon, MailUiProvider, useMailUi, type MailReaderUi } from '../ui'
import { Composer, type ComposeMode, type ComposeValues } from './Composer'
import { EmailList } from './EmailList'
import type { MessageActions, RenderHtml } from './EmailViewer'
import { FolderList } from './FolderList'
import { InboxList } from './InboxList'
import { LIST, NARROW_BELOW, PaneHandle, SIDEBAR, useElementWidth, usePaneWidths } from './panes'
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
  /** The host's controls and icons (see ../ui); plain ones by default. */
  ui?: Partial<MailReaderUi>
  /** Shown at the top of an open conversation, under its subject: an agent's
   *  unsent reply (DraftCard). Nothing feeds it yet; see DraftCard. */
  threadTop?: (messages: ReaderMessage[]) => React.ReactNode
}

/** Which column a narrow reader shows. */
export type NarrowView = 'folders' | 'list' | 'message'

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

export function MailReader({ ui, ...rest }: MailReaderProps): React.JSX.Element {
  return (
    <MailUiProvider ui={ui}>
      <Reader {...rest} />
    </MailUiProvider>
  )
}

function Reader({
  source,
  renderHtml,
  confirm = defaultConfirm,
  saveAttachment = defaultSave,
  pageSize = 50,
  banner,
  threadTop,
}: Omit<MailReaderProps, 'ui'>): React.JSX.Element {
  const kit = useMailUi()
  const [rootRef, width] = useElementWidth<HTMLDivElement>()
  const narrow = width > 0 && width < NARROW_BELOW
  const [panes, setPanes] = usePaneWidths(width)
  const [view, setView] = useState<NarrowView>('list')
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

  const lastGroup = useRef<ThreadGroup | null>(null)
  const openThread = useCallback(
    async (group: ThreadGroup) => {
      lastGroup.current = group
      const id = group.latestEmail.id
      const seq = ++threadSeq.current
      setSelectedId(id)
      setCompose(null)
      setNotice(null)
      setView('message')
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
    onReplyAll: (msg) => {
      const others = [...(msg.to ?? []), ...(msg.cc ?? [])].map((a) => a.address).filter((a) => a && a !== msg.inbox && a !== msg.from?.address)
      setCompose({
        kind: 'reply',
        all: true,
        inbox: msg.inbox,
        id: msg.id,
        subject: msg.subject || '',
        to: [msg.from?.address || '', ...others].filter(Boolean).join(', '),
      })
    },
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
          ? source.reply(mode.all ? { id: mode.id, body: v.body, all: true } : { id: mode.id, body: v.body })
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

  const pickFolder = (f: string): void => {
    setFolder(f)
    setSelectedId(null)
    setThread(null)
    setThreadError(null)
    setView('list')
  }
  const startNew = (): void => {
    if (!inbox) return
    setNotice(null)
    setCompose({ kind: 'new', inbox })
    setView('message')
  }
  const backTo = (v: NarrowView): void => {
    if (v === 'list') setCompose(null)
    setView(v)
  }

  /** A narrow column's top row: Back to the column before, and its title. */
  const backBar = (to: NarrowView, label: string, title?: React.ReactNode): React.ReactNode =>
    narrow ? (
      <div data-mr="back-bar" className="flex items-center gap-2 px-2 py-1.5 border-b border-[var(--color-border)] min-h-[40px]">
        <IconButton icon="back" label={label} aria-label={`${t('back')}: ${label}`} showLabel data-testid="mr-back" onClick={() => backTo(to)} />
        {title && <span className="ml-auto min-w-0 truncate type-name text-[var(--color-text-primary)] pr-2">{title}</span>}
      </div>
    ) : null

  const sidebar = (
    <aside
      data-mr="sidebar"
      {...kit.shape('pane')}
      className={`flex flex-col min-h-0 bg-[var(--color-bg-surface)] ${narrow ? 'flex-1' : 'flex-shrink-0 border-r border-[var(--color-border)]'}`}
      style={narrow ? undefined : { width: panes.sidebar }}
    >
      <div className="flex-1 min-h-0 overflow-y-auto">
        {inboxError && (
          <p data-testid="mr-inbox-error" role="alert" className="px-4 py-2 type-hint text-[var(--color-status-error-text)]">
            {inboxError}
          </p>
        )}
        <InboxList
          viewer={viewer}
          inboxes={inboxes}
          selected={inbox}
          onSelect={(a) => {
            setInbox(a)
          }}
        />
        {inbox && folders.length > 0 && (
          <div className="mx-4 border-t border-[var(--color-border)]" aria-hidden="true" />
        )}
        {inbox && folders.length > 0 && <FolderList folders={folders} selected={folder} onSelect={pickFolder} />}
      </div>
    </aside>
  )

  const listPane = (
    <section
      data-mr="list-pane"
      aria-label={t('conversations')}
      {...kit.shape('pane')}
      className={`flex flex-col min-h-0 ${narrow ? 'flex-1' : 'flex-shrink-0 border-r border-[var(--color-border)]'}`}
      style={narrow ? undefined : { width: panes.list }}
    >
      {backBar('folders', t('mailboxes'), currentInbox?.displayName || inbox)}
      <form
        role="search"
        className="flex items-center gap-1.5 px-3 pt-2.5 pb-2"
        onSubmit={(e) => {
          e.preventDefault()
          setAppliedQuery(query.trim())
        }}
      >
        <kit.SearchField
          data-testid="mr-search"
          aria-label={t('search_placeholder')}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t('search_placeholder')}
          className="flex-1 min-w-0"
        />
        <IconButton icon="refresh" label={t('refresh')} data-testid="mr-refresh" onClick={refresh} />
        {inbox && <IconButton icon="compose" label={t('new_message')} variant="primary" data-testid="mr-new-message" disabled={busy} onClick={startNew} />}
      </form>
      <div className="flex items-center gap-2 px-4 pb-2 border-b border-[var(--color-border)]">
        {!narrow && inbox && (
          <span className="flex-1 min-w-0 truncate type-name text-[var(--color-text-primary)]" title={inbox}>
            {currentInbox?.displayName || inbox}
          </span>
        )}
        <label className={`flex items-center gap-1.5 type-hint text-[var(--color-text-secondary)] cursor-pointer ${narrow || !inbox ? 'ml-0' : 'ml-auto'}`}>
          <input
            type="checkbox"
            data-testid="mr-unread-only"
            className="accent-[var(--color-accent)]"
            checked={unreadOnly}
            onChange={(e) => setUnreadOnly(e.target.checked)}
          />
          {t('unread_only')}
        </label>
      </div>
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
          onRetry={() => void loadList()}
        />
      ) : (
        <p className="px-4 py-6 type-row text-[var(--color-text-muted)]">{t('pick_inbox')}</p>
      )}
    </section>
  )

  const messagePane = (
    <section
      data-mr="message-pane"
      {...kit.shape('pane')}
      // Size containment: the HTML paper is as tall as this pane's view
      // (100cqh), and the composer's hint follows its width.
      className="flex-1 min-w-0 min-h-0 flex flex-col [container-type:size]"
    >
      {backBar('list', t('conversations'))}
      <div className="flex-1 min-h-0 overflow-y-auto">
        {notice && (
          <div
            data-mr="notice"
            role="status"
            {...kit.shape('card')}
            className="mx-5 mt-4 flex items-center gap-2 pl-3 pr-1 py-1 border border-[var(--color-border)] rounded-[var(--radius-field)] bg-[var(--color-bg-elevated)]"
          >
            <p data-testid="mr-notice" className="flex-1 type-row text-[var(--color-text-primary)]">
              {notice}
            </p>
            <IconButton icon="close" label={t('dismiss')} onClick={() => setNotice(null)} />
          </div>
        )}
        {compose && (
          <Composer
            key={compose.kind === 'reply' ? `r:${compose.id}:${compose.all ? 'all' : 'one'}` : `n:${compose.inbox}`}
            mode={compose}
            busy={busy}
            confirm={confirm}
            onSend={(v) => void send(compose, v)}
            onSaveDraft={(v) => void saveDraft(compose, v)}
            onCancel={() => {
              setCompose(null)
              if (!thread) setView('list')
            }}
          />
        )}
        {threadLoading && (
          <div data-mr="thread-loading" role="status" aria-busy="true" className="px-5 py-5 flex flex-col gap-3">
            <span className="type-hint text-[var(--color-text-muted)]">{t('thread_loading')}</span>
            <span className="h-4 w-3/5 rounded-[var(--radius-badge)] bg-[var(--color-bg-elevated)]" />
            <span className="h-3 w-2/5 rounded-[var(--radius-badge)] bg-[var(--color-bg-elevated)]" />
            <span className="mt-2 h-24 w-full rounded-[var(--radius-box)] bg-[var(--color-bg-elevated)]" />
          </div>
        )}
        {threadError && (
          <div role="alert" className="px-5 py-10 flex flex-col items-center text-center gap-1">
            <p className="type-row text-[var(--color-status-error-text)]">{t('thread_error_title')}</p>
            <p data-testid="mr-thread-error" className="type-hint text-[var(--color-text-muted)] max-w-[48ch]">
              {threadError}
            </p>
            {lastGroup.current && (
              <kit.Button variant="default" className="mt-2" onClick={() => lastGroup.current && void openThread(lastGroup.current)}>
                {t('try_again')}
              </kit.Button>
            )}
          </div>
        )}
        {!threadLoading && !threadError && thread && (
          <ThreadConversationView messages={thread} renderHtml={renderHtml} actionsFor={actionsFor} top={threadTop?.(thread)} />
        )}
        {!threadLoading && !threadError && !thread && !compose && inbox && (
          <div data-mr="message-empty" className="h-full min-h-[240px] px-6 flex flex-col items-center justify-center text-center gap-1">
            <MailIcon name="inbox" size={28} className="mb-2 text-[var(--color-text-muted)]" />
            <p className="type-row text-[var(--color-text-primary)]">{t('pick_message_title')}</p>
            <p className="type-hint text-[var(--color-text-muted)] max-w-[40ch]">{t('pick_message_hint')}</p>
          </div>
        )}
      </div>
    </section>
  )

  return (
    <div
      ref={rootRef}
      data-mr="reader"
      data-testid="mr-reader"
      data-layout={narrow ? 'stacked' : 'columns'}
      data-view={narrow ? view : undefined}
      className="flex flex-col h-full min-h-0 text-[var(--color-text-primary)]"
    >
      {banner}
      <div data-mr="columns" className="flex flex-1 min-h-0 gap-[var(--gap-pane)] p-[var(--gap-pane)]">
        {narrow ? (
          view === 'folders' ? (
            sidebar
          ) : view === 'list' || (!compose && !thread && !threadLoading && !threadError) ? (
            listPane
          ) : (
            messagePane
          )
        ) : (
          <>
            {sidebar}
            <PaneHandle
              label={t('resize_sidebar')}
              testId="mr-resize-sidebar"
              value={panes.sidebar}
              min={SIDEBAR.min}
              max={SIDEBAR.max}
              initial={SIDEBAR.initial}
              onChange={(n) => setPanes({ ...panes, sidebar: n })}
            />
            {listPane}
            <PaneHandle
              label={t('resize_list')}
              testId="mr-resize-list"
              value={panes.list}
              min={LIST.min}
              max={LIST.max}
              initial={LIST.initial}
              onChange={(n) => setPanes({ ...panes, list: n })}
            />
            {messagePane}
          </>
        )}
      </div>
    </div>
  )
}
