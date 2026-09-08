import { useState, useEffect, useCallback, useMemo } from 'react'
import { listen } from '@tauri-apps/api/event'
import { agentDisplayName } from '@/lib/workspace-agent'
import { daemonCliGet, isHostSwitchedError } from '@/lib/daemon-cli'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'
import {
  MAIL_PAGE_LIMIT,
  TRAY_SOURCE_ID,
  buildSourceList,
  formatInboxError,
  formatMailDate,
  formatMailFrom,
  mailHtmlSrcDoc,
  parseFolderList,
  parseMailCatalog,
  parseMailMessages,
  parseMailRead,
  parseTrayList,
  parseTrayRead,
  mailHtmlIsEmpty,
  mailTextIsEmpty,
  preferMailHtml,
  stripExternalEmailMarkers,
  trayFolderLabel,
  trayFolderOptions,
  traySource,
  type InboxBrowserSource,
  type InboxItem,
  type MailMessageFull,
  type MailMessageSummary,
} from './inbox-browser'

interface AgentInboxPaneProps {
  agentName: string
  projectPath: string
}

type SelectedRow =
  | { kind: 'tray'; id: string }
  | { kind: 'mail'; id: string }
  | null

function shouldIgnoreFetchError(err: unknown): boolean {
  return isHostSwitchedError(err)
}

/**
 * Pinned Inbox / Work Board tab — view-only three-pane browser
 * (sources → messages → body). Replaces the pre-v1 kanban. Sibling tab
 * is `AgentChatPane`; both are pinned by `tabs.ts`.
 */
export function AgentInboxPane({ agentName, projectPath }: AgentInboxPaneProps): React.JSX.Element {
  const isWorkspaceBoard = agentName === '__workspace__'

  const [displayName, setDisplayName] = useState<string>(agentName)
  const [mailSources, setMailSources] = useState<InboxBrowserSource[]>([])
  const [catalogError, setCatalogError] = useState<string | null>(null)
  const [catalogLoaded, setCatalogLoaded] = useState(false)

  const [selectedSourceId, setSelectedSourceId] = useState<string>(TRAY_SOURCE_ID)
  const [trayFolder, setTrayFolder] = useState<string>('')
  const [folders, setFolders] = useState<string[]>([])

  const [trayItems, setTrayItems] = useState<InboxItem[]>([])
  const [mailMessages, setMailMessages] = useState<MailMessageSummary[]>([])
  const [mailNextOffset, setMailNextOffset] = useState<number | null>(null)
  const [listError, setListError] = useState<string | null>(null)
  const [listLoading, setListLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)

  const [selectedRow, setSelectedRow] = useState<SelectedRow>(null)
  const [trayBody, setTrayBody] = useState<string | null>(null)
  const [mailBody, setMailBody] = useState<MailMessageFull | null>(null)
  const [bodyError, setBodyError] = useState<string | null>(null)
  const [bodyLoading, setBodyLoading] = useState(false)
  const [showHtml, setShowHtml] = useState(false)

  const sources = useMemo(() => buildSourceList(mailSources), [mailSources])
  const selectedSource = sources.find((s) => s.id === selectedSourceId) ?? traySource()
  const selectedKind = selectedSource.kind
  const selectedAddress = selectedSource.address
  const isTray = selectedKind === 'tray'
  const folderChips = useMemo(() => trayFolderOptions(folders), [folders])

  const fetchCatalog = useCallback(async () => {
    if (!projectPath) return
    setCatalogLoaded(false)
    try {
      const raw = await daemonCliGet<unknown>('mail/inboxes', { project: projectPath })
      setMailSources(parseMailCatalog(raw))
      setCatalogError(null)
    } catch (err) {
      if (shouldIgnoreFetchError(err)) return
      setCatalogError(formatInboxError(err))
    } finally {
      setCatalogLoaded(true)
    }
  }, [projectPath])

  const fetchFolders = useCallback(async () => {
    if (!projectPath) return
    try {
      const raw = await daemonCliGet<unknown>('inbox/folders', { project: projectPath })
      setFolders(parseFolderList(raw))
    } catch (err) {
      if (shouldIgnoreFetchError(err)) return
      // Folder chips still work with the built-in Inbox/active/done set.
      setFolders([])
    }
  }, [projectPath])

  const fetchTrayList = useCallback(async () => {
    if (!projectPath) return
    setListLoading(true)
    try {
      const raw = await daemonCliGet<unknown>('inbox/list', {
        project: projectPath,
        folder: trayFolder,
      })
      setTrayItems(parseTrayList(raw))
      setListError(null)
    } catch (err) {
      if (shouldIgnoreFetchError(err)) return
      setListError(formatInboxError(err))
    } finally {
      setListLoading(false)
    }
  }, [projectPath, trayFolder])

  const fetchMailList = useCallback(async (opts?: { offset?: number; append?: boolean }) => {
    if (!projectPath || selectedKind === 'tray' || !selectedAddress) return
    const offset = opts?.offset ?? 0
    const append = opts?.append === true
    if (append) setLoadingMore(true)
    else setListLoading(true)
    try {
      const raw = await daemonCliGet<unknown>('mail/messages', {
        project: projectPath,
        address: selectedAddress,
        limit: MAIL_PAGE_LIMIT,
        offset,
      })
      const page = parseMailMessages(raw)
      setMailMessages((prev) => {
        if (!append) return page.messages
        const seen = new Set(prev.map((m) => m.id))
        return [...prev, ...page.messages.filter((m) => !seen.has(m.id))]
      })
      setMailNextOffset(page.nextOffset)
      setListError(page.inboxErrors.length > 0 ? page.inboxErrors.join(' · ') : null)
    } catch (err) {
      if (shouldIgnoreFetchError(err)) return
      setListError(formatInboxError(err))
      if (!append) setMailNextOffset(null)
    } finally {
      setListLoading(false)
      setLoadingMore(false)
    }
  }, [projectPath, selectedKind, selectedAddress])

  const fetchList = useCallback(async () => {
    if (isTray) await fetchTrayList()
    else await fetchMailList()
  }, [isTray, fetchTrayList, fetchMailList])

  useEffect(() => {
    void fetchCatalog()
  }, [fetchCatalog])

  useEffect(() => {
    if (isTray) void fetchFolders()
  }, [isTray, fetchFolders])

  useEffect(() => {
    setSelectedRow(null)
    setTrayBody(null)
    setMailBody(null)
    setBodyError(null)
    setShowHtml(false)
    setMailMessages([])
    setMailNextOffset(null)
    setListError(null)
    void fetchList()
  }, [fetchList])

  useEffect(() => {
    if (!isTray) return
    const interval = setInterval(() => { void fetchTrayList() }, 10_000)
    return () => clearInterval(interval)
  }, [isTray, fetchTrayList])

  useEffect(() => {
    let cancelled = false
    if (isWorkspaceBoard) {
      setDisplayName(agentName)
      return
    }
    agentDisplayName(projectPath)
      .then((n) => { if (!cancelled && n) setDisplayName(n) })
      .catch(() => { /* keep agentName as fallback */ })
    return () => { cancelled = true }
  }, [projectPath, agentName, isWorkspaceBoard])

  useEffect(() => {
    if (isWorkspaceBoard) return
    let unlisten: (() => void) | null = null
    let cancelled = false
    listen('sync:projects', () => {
      agentDisplayName(projectPath)
        .then((n) => { if (n) setDisplayName(n) })
        .catch(() => {})
    }).then((u) => { if (cancelled) u(); else unlisten = u })
    return () => { cancelled = true; unlisten?.() }
  }, [projectPath, isWorkspaceBoard])

  const openTrayItem = async (item: InboxItem): Promise<void> => {
    setSelectedRow({ kind: 'tray', id: item.id })
    setMailBody(null)
    setShowHtml(false)
    setBodyLoading(true)
    setBodyError(null)
    try {
      const raw = await daemonCliGet<unknown>('inbox/read', {
        project: projectPath,
        id: item.id,
      })
      setTrayBody(parseTrayRead(raw).content)
    } catch (err) {
      if (shouldIgnoreFetchError(err)) return
      setTrayBody(null)
      setBodyError(formatInboxError(err))
    } finally {
      setBodyLoading(false)
    }
  }

  const openMailItem = async (item: MailMessageSummary): Promise<void> => {
    setSelectedRow({ kind: 'mail', id: item.id })
    setTrayBody(null)
    setShowHtml(false)
    setBodyLoading(true)
    setBodyError(null)
    try {
      const raw = await daemonCliGet<unknown>('mail/read', {
        project: projectPath,
        id: item.id,
        html: 1,
      })
      const message = parseMailRead(raw)
      setMailBody(message)
      setMailMessages((prev) =>
        prev.map((m) => (m.id === item.id ? { ...m, unread: false } : m)),
      )
    } catch (err) {
      if (shouldIgnoreFetchError(err)) return
      setMailBody(null)
      setBodyError(formatInboxError(err))
    } finally {
      setBodyLoading(false)
    }
  }

  const loadMore = (): void => {
    if (mailNextOffset == null || isTray) return
    void fetchMailList({ offset: mailNextOffset, append: true })
  }

  const selectSource = (id: string): void => {
    if (id === selectedSourceId) return
    setSelectedSourceId(id)
    setListError(null)
    setTrayFolder('')
  }

  const headerLabel = isWorkspaceBoard ? 'Work Board' : displayName
  const emptyCatalog = catalogLoaded && mailSources.length === 0 && !catalogError

  return (
    <div
      className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden"
      data-testid="inbox-browser"
    >
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-3">
        <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
          {headerLabel}
        </span>
      </div>

      <div className="flex-1 min-h-0 flex">
        <SourceColumn
          sources={sources}
          selectedId={selectedSourceId}
          catalogError={catalogError}
          emptyCatalog={emptyCatalog}
          onSelect={selectSource}
        />
        <MessageColumn
          isTray={isTray}
          folder={trayFolder}
          folders={folderChips}
          onFolder={setTrayFolder}
          trayItems={trayItems}
          mailMessages={mailMessages}
          selectedRow={selectedRow}
          listError={listError}
          listLoading={listLoading}
          nextOffset={isTray ? null : mailNextOffset}
          loadingMore={loadingMore}
          onOpenTray={openTrayItem}
          onOpenMail={openMailItem}
          onLoadMore={loadMore}
        />
        <BodyColumn
          selectedRow={selectedRow}
          trayBody={trayBody}
          mailBody={mailBody}
          bodyError={bodyError}
          bodyLoading={bodyLoading}
          showHtml={showHtml}
          onShowHtml={setShowHtml}
        />
      </div>
    </div>
  )
}

function SourceColumn({
  sources,
  selectedId,
  catalogError,
  emptyCatalog,
  onSelect,
}: {
  sources: InboxBrowserSource[]
  selectedId: string
  catalogError: string | null
  emptyCatalog: boolean
  onSelect: (id: string) => void
}): React.JSX.Element {
  return (
    <div
      className="w-48 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0"
      data-testid="inbox-source-list"
    >
      <div className="px-3 py-2 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
        Inboxes
      </div>
      {catalogError && (
        <div
          className="mx-2 mb-2 px-2 py-1 text-[11px] text-[var(--color-status-error-soft)]"
          data-testid="inbox-catalog-error"
          role="alert"
        >
          {catalogError}
        </div>
      )}
      <div className="flex-1 overflow-y-auto px-1 pb-2">
        {sources.map((source) => {
          const selected = source.id === selectedId
          return (
            <button
              key={source.id}
              type="button"
              data-testid={source.kind === 'tray' ? 'inbox-source-tray' : `inbox-source-mail-${source.address}`}
              data-kind={source.kind}
              onClick={() => onSelect(source.id)}
              className={`w-full text-left px-2 py-1.5 mb-0.5 cursor-pointer ${
                selected
                  ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-primary)] hover:bg-[var(--color-wash-1)]'
              }`}
            >
              <div className="text-xs font-medium truncate">{source.label}</div>
              {source.tag && (
                <div className="text-[9px] uppercase tracking-wide text-[var(--color-text-muted)] mt-0.5">
                  {source.tag}
                </div>
              )}
            </button>
          )
        })}
        {emptyCatalog && (
          <p className="px-2 pt-2 text-[11px] text-[var(--color-text-muted)] leading-relaxed">
            No mail inboxes. Link or host one in Settings → Email.
          </p>
        )}
      </div>
    </div>
  )
}

function MessageColumn({
  isTray,
  folder,
  folders,
  onFolder,
  trayItems,
  mailMessages,
  selectedRow,
  listError,
  listLoading,
  nextOffset,
  loadingMore,
  onOpenTray,
  onOpenMail,
  onLoadMore,
}: {
  isTray: boolean
  folder: string
  folders: string[]
  onFolder: (folder: string) => void
  trayItems: InboxItem[]
  mailMessages: MailMessageSummary[]
  selectedRow: SelectedRow
  listError: string | null
  listLoading: boolean
  nextOffset: number | null
  loadingMore: boolean
  onOpenTray: (item: InboxItem) => void
  onOpenMail: (item: MailMessageSummary) => void
  onLoadMore: () => void
}): React.JSX.Element {
  const empty = isTray ? trayItems.length === 0 : mailMessages.length === 0
  return (
    <div
      className="w-72 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0"
      data-testid="inbox-message-list"
    >
      <div className="px-3 py-2 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
        Messages
      </div>
      {isTray && (
        <div className="px-2 pb-2 flex flex-wrap gap-1" data-testid="inbox-folder-filter">
          {folders.map((f) => {
            const selected = f === folder
            return (
              <button
                key={f || 'inbox'}
                type="button"
                data-testid={`inbox-folder-${f || 'inbox'}`}
                onClick={() => onFolder(f)}
                className={`text-[10px] px-1.5 py-0.5 cursor-pointer ${
                  selected
                    ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)]'
                    : 'bg-[var(--color-wash-1)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
                }`}
              >
                {trayFolderLabel(f)}
              </button>
            )
          })}
        </div>
      )}
      {listError && (
        <div
          className="mx-2 mb-2 px-2 py-1 text-[11px] text-[var(--color-status-error-soft)]"
          data-testid="inbox-list-error"
          role="alert"
        >
          {listError}
        </div>
      )}
      <div className="flex-1 overflow-y-auto px-1 pb-2">
        {isTray
          ? trayItems.map((item) => {
              const selected = selectedRow?.kind === 'tray' && selectedRow.id === item.id
              return (
                <button
                  key={item.id}
                  type="button"
                  data-testid={`inbox-message-${item.id}`}
                  onClick={() => onOpenTray(item)}
                  className={`w-full text-left px-2 py-2 mb-0.5 cursor-pointer ${
                    selected
                      ? 'bg-[var(--color-accent)]/15'
                      : 'hover:bg-[var(--color-wash-1)]'
                  }`}
                >
                  <div className="text-xs font-medium text-[var(--color-text-primary)] leading-snug truncate">
                    {item.title || item.filename}
                  </div>
                  <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 truncate">
                    {item.from}
                    {item.created ? ` · ${item.created}` : ''}
                  </div>
                  {item.bodyPreview && (
                    <div className="text-[10px] text-[var(--color-text-muted)] mt-1 line-clamp-2">
                      {item.bodyPreview}
                    </div>
                  )}
                </button>
              )
            })
          : mailMessages.map((item) => {
              const selected = selectedRow?.kind === 'mail' && selectedRow.id === item.id
              const from = formatMailFrom(item.from)
              return (
                <button
                  key={item.id}
                  type="button"
                  data-testid={`inbox-message-${item.id}`}
                  data-unread={item.unread ? 'true' : 'false'}
                  onClick={() => onOpenMail(item)}
                  className={`w-full text-left px-2 py-2 mb-0.5 cursor-pointer ${
                    selected
                      ? 'bg-[var(--color-accent)]/15'
                      : 'hover:bg-[var(--color-wash-1)]'
                  }`}
                >
                  <div className="flex items-start gap-1.5">
                    {item.unread && (
                      <span className="mt-1.5 w-1.5 h-1.5 rounded-full bg-[var(--color-accent)] flex-shrink-0" />
                    )}
                    <div className="min-w-0 flex-1">
                      <div className="text-xs font-medium text-[var(--color-text-primary)] leading-snug truncate">
                        {item.subject || '(no subject)'}
                      </div>
                      <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 truncate">
                        {from}
                        {item.date ? ` · ${formatMailDate(item.date)}` : ''}
                      </div>
                    </div>
                  </div>
                </button>
              )
            })}
        {empty && !listError && !listLoading && (
          <div className="px-3 py-6 text-[11px] text-[var(--color-text-muted)] text-center">
            {isTray ? 'No packages' : 'No messages'}
          </div>
        )}
        {nextOffset != null && (
          <button
            type="button"
            data-testid="inbox-load-more"
            onClick={onLoadMore}
            disabled={loadingMore}
            className="w-full mt-1 px-2 py-1.5 text-[11px] text-[var(--color-accent)] hover:bg-[var(--color-wash-1)] cursor-pointer"
          >
            {loadingMore ? 'Loading…' : 'Load more'}
          </button>
        )}
      </div>
    </div>
  )
}

function BodyColumn({
  selectedRow,
  trayBody,
  mailBody,
  bodyError,
  bodyLoading,
  showHtml,
  onShowHtml,
}: {
  selectedRow: SelectedRow
  trayBody: string | null
  mailBody: MailMessageFull | null
  bodyError: string | null
  bodyLoading: boolean
  showHtml: boolean
  onShowHtml: (v: boolean) => void
}): React.JSX.Element {
  const useHtml = mailBody
    ? preferMailHtml(mailBody.text, mailBody.html, showHtml)
    : false
  const htmlHasContent = Boolean(mailBody && !mailHtmlIsEmpty(mailBody.html))
  const textHasContent = Boolean(mailBody && !mailTextIsEmpty(mailBody.text))
  const showToggle = htmlHasContent && textHasContent

  return (
    <div className="flex-1 min-w-0 flex flex-col min-h-0" data-testid="inbox-body">
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-2">
        <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
          Body
        </span>
        {selectedRow?.kind === 'mail' && (
          <span className="text-[10px] text-[var(--color-text-muted)]">Marks read</span>
        )}
        {showToggle && (
          <button
            type="button"
            data-testid="inbox-html-toggle"
            onClick={() => onShowHtml(!showHtml)}
            className="ml-auto text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer"
          >
            {useHtml ? 'Show text' : 'Show HTML'}
          </button>
        )}
      </div>
      {bodyError && (
        <div
          className="mx-3 mt-2 px-2 py-1 text-[11px] text-[var(--color-status-error-soft)]"
          data-testid="inbox-body-error"
          role="alert"
        >
          {bodyError}
        </div>
      )}
      <div className="flex-1 overflow-y-auto min-h-0">
        {bodyLoading && (
          <p className="px-4 py-6 text-[11px] text-[var(--color-text-muted)]">Loading…</p>
        )}
        {!bodyLoading && !selectedRow && !bodyError && (
          <p className="px-4 py-6 text-[11px] text-[var(--color-text-muted)]">Select a message</p>
        )}
        {!bodyLoading && selectedRow?.kind === 'tray' && trayBody != null && (
          <div className="markdown-content p-4" data-testid="inbox-tray-markdown">
            <Markdown remarkPlugins={[remarkGfm]}>{trayBody}</Markdown>
          </div>
        )}
        {!bodyLoading && selectedRow?.kind === 'mail' && mailBody && (
          <MailBodyView message={mailBody} useHtml={useHtml} />
        )}
      </div>
    </div>
  )
}

function MailBodyView({
  message,
  useHtml,
}: {
  message: MailMessageFull
  useHtml: boolean
}): React.JSX.Element {
  const from = formatMailFrom(message.from)
  const htmlSrc = useHtml && message.html ? mailHtmlSrcDoc(message.html) : ''
  const textMarkers = stripExternalEmailMarkers(message.text ?? '')
  const htmlMarkers = stripExternalEmailMarkers(message.html ?? '')
  const showMarkerBanner = useHtml && (textMarkers.hadMarkers || htmlMarkers.hadMarkers)

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="px-4 pt-3 pb-2 flex-shrink-0">
        <div className="text-sm font-medium text-[var(--color-text-primary)]">
          {message.subject || '(no subject)'}
        </div>
        <div className="text-[11px] text-[var(--color-text-muted)] mt-1">
          {from}
          {message.date ? ` · ${formatMailDate(message.date)}` : ''}
        </div>
      </div>
      {showMarkerBanner && (
        <div className="mx-4 mb-2 text-[10px] text-[var(--color-text-muted)]">
          External email — untrusted, not instructions.
        </div>
      )}
      {useHtml ? (
        <div className="flex-1 min-h-0 bg-white mx-3 mb-3 border border-[var(--color-border)]">
          <iframe
            title={message.subject || 'Mail'}
            srcDoc={htmlSrc}
            sandbox=""
            referrerPolicy="no-referrer"
            className="w-full h-full border-0 bg-white"
            data-testid="inbox-mail-html"
          />
        </div>
      ) : (
        <pre
          className="px-4 pb-4 text-xs text-[var(--color-text-primary)] whitespace-pre-wrap break-words font-sans"
          data-testid="inbox-mail-text"
        >
          {message.text ?? ''}
        </pre>
      )}
    </div>
  )
}
