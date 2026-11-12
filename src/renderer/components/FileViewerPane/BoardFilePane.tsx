// A `.k5board` in a K2 tab (prd-board-tab-v1 §4; board-tab-b3-interface.md). The canvas is `k2:board-file@2` in a sealed
// frame; everything around it (the chip, the banners, the cards) is K2's own DOM, because a file can come from anyone.
// The truth lives in the daemon, and what can't reach it yet lives in the session (lib/board-file), not in this pane.
//
// Clients are thin: nothing here reads or writes the format, and opening a board sends no write.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { HtmlFrame } from '@/components/HtmlFrame/HtmlFrame'
import { useRoom, useRoomTabs } from '@/components/Room/RoomContext'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { isWebClient } from '@/lib/is-web'
import { useToastStore } from '@/stores/toast'
import { fileUndoFor } from '@/stores/file-undo'
import { onBoardFileChanged, onFsChanged } from '@/stores/session-events'
import { fetchZenWidgetBundle, zenWidgetError } from '@/lib/zen/zen-widget-routes'
import { zenLib } from '@/lib/zen/zen-lib-loader'
import { zenFramePrelude } from '@/lib/zen/zen-custom-prelude'
import { ZEN_PICK_GESTURE_MS, ZenCustomRefusal, createZenCustomLayer } from '@/lib/zen/zen-custom-bridge'
import { startZenFrameHost } from '@/lib/zen/zen-custom-host'
import { ZenUploadsError, zenPick } from '@/lib/zen/zen-custom-uploads'
import { pickBrowserFiles } from '@/lib/zen/zen-thread-files'
import { allowZenLinkSite, createZenLinks, fetchZenLinkSites, type ZenLinkAnswer, type ZenLinkAsk } from '@/lib/zen/zen-links'
import { daemonZenDownloadConsent, spendZenDownload, zenDownloadConsent, type ZenDownloadAnswer, type ZenDownloadAsk } from '@/lib/zen/zen-download-consent'
import { registerZenCanvasFrame, zenCanvasPointer, zenCanvasPressedWithin } from '@/lib/zen/zen-canvas'
import { zenFrameHasFocus } from '@/lib/zen/zen-shortcut'
import { downloadChunks } from '@/lib/fs-transfer'
import { openUrl } from '@tauri-apps/plugin-opener'
import { ZenLinkSheet } from '@/components/Zen/widgets/ZenLinkSheet'
import { ZenDownloadSheet } from '@/components/Zen/widgets/ZenDownloadSheet'
import type { ZenCustomWidgetPayload, ZenFrameHello } from '@/lib/zen/zen-custom-types'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { createBoardFilePartJoiner, type BoardFileApi, type BoardFileSession, type BoardFileView, type BoardOp, type HeldGroup } from '@/lib/board-file/board-file-session'
import { createBoardFileBlobs } from '@/lib/board-file/board-file-blobs'
import { acquireBoardFileSession, askBeforeBoardFileLoss, releaseBoardFileSession } from '@/lib/board-file/board-file-tabs'
import { BOARD_FILE_FEATURES, BOARD_FILE_WIDGET, boardFileTheme, boardFileConsentKey, redispatchBoardFileKey } from '@/lib/board-file/board-file-frame'
import { FILE_POLL_INTERVAL } from '@shared/constants'
import { BoardWebStub } from './BoardWebStub'
import { BoardFileCard } from './BoardFileCard'
import { BoardFileName } from './BoardFileName'

export interface BoardFilePaneProps {
  filePath: string
  paneId?: string
  paneGroupId?: string
  tabId?: string
}

const fmtBytes = (n: number): string => (n >= 1024 * 1024 ? `${Math.round(n / (1024 * 1024))} MiB` : `${Math.ceil(n / 1024)} KiB`)
const baseName = (p: string): string => p.split(/[\\/]/).pop() ?? p
const dirName = (p: string): string => p.slice(0, Math.max(0, p.lastIndexOf('/')))

function Card({ title, children }: { title: string; children?: React.ReactNode }): React.JSX.Element {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-xs text-[var(--color-text-muted)]" data-board-file-card="">
      <div className="text-[var(--color-text-primary)]">{title}</div>
      {children}
    </div>
  )
}
function CardButton(props: { onClick(): void; children: React.ReactNode; testid?: string }): React.JSX.Element {
  return (
    <button type="button" data-testid={props.testid} onClick={props.onClick} className="h-6 cursor-pointer border border-[var(--color-border)] px-2 text-xs text-[var(--color-text-primary)]">
      {props.children}
    </button>
  )
}

export function BoardFilePane(props: BoardFilePaneProps): React.JSX.Element {
  if (isWebClient()) return <BoardWebStub />
  if (!props.paneGroupId) return <OpenInATab filePath={props.filePath} />
  return <BoardFilePaneLive {...props} />
}

function OpenInATab({ filePath }: { filePath: string }): React.JSX.Element {
  const openFileAsTab = useRoomTabs((s) => s.openFileAsTab)
  return (
    <Card title="This board opens in a tab">
      <CardButton onClick={() => openFileAsTab(filePath)}>Open in a tab</CardButton>
    </Card>
  )
}

function BoardFilePaneLive({ filePath, tabId }: BoardFilePaneProps): React.JSX.Element {
  const room = useRoom()
  const scope = room.scope
  const closeTab = useRoomTabs((s) => s.removeTab)
  const setTabDirty = useRoomTabs((s) => s.setTabDirty)
  const consumeDiscardPending = useRoomTabs((s) => s.consumeDiscardPending)
  const consumeSaveConfirmed = useRoomTabs((s) => s.consumeSaveConfirmed)
  const boxRef = useRef<HTMLDivElement | null>(null)

  const api: BoardFileApi = useMemo(
    () => ({
      get: (route, params) => daemonCliGet(scope, route, params),
      post: (route, body) => daemonCliPost(scope, route, body),
    }),
    [scope],
  )
  const session = useMemo(
    () =>
      acquireBoardFileSession({
        connectionKey: scope.connectionKey,
        path: filePath,
        api,
        visible: () => document.visibilityState === 'visible',
      }),
    // A pane is keyed by its path, and owns its acquire until unmount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  )
  const [view, setView] = useState<BoardFileView>(() => session.view())
  useEffect(() => session.subscribe(() => setView(session.view())), [session])

  useEffect(() => {
    if (session.view().phase.kind === 'loading') void session.start()
    return () => {
      const discard = tabId ? consumeDiscardPending(tabId) : false
      if (tabId) consumeSaveConfirmed(tabId)
      void releaseBoardFileSession(scope.connectionKey, filePath, { discard })
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // The leave guard asks about what only this tab has: queued edits, and held ones a daemon isn't keeping. One the
  // daemon keeps shows on the chip and comes back on the next open.
  const unsaved = view.queued + view.held.filter((h) => h.heldId === null).length
  useEffect(() => {
    if (tabId) setTabDirty(tabId, unsaved > 0)
  }, [tabId, unsaved, setTabDirty])

  // What changes the file: K2's own saves by name, everything else as a change on disk, and a poll as the backstop
  // (`fs_live` skips .k2/, so a board under .k2/wiki, prds or docs has only this).
  useEffect(() => {
    const offBoard = onBoardFileChanged(scope, (e) => {
      const joined = `${e.workspace.replace(/\/+$/, '')}/${e.rel}`
      if (joined === filePath) session.onBoardChanged(e.hash)
    })
    const offFs = onFsChanged(scope, (e) => {
      if (e.paths.includes(filePath)) session.onOutsideChange()
    })
    const poll = window.setInterval(() => {
      if (document.visibilityState === 'visible' && document.hasFocus()) void session.pollInfo()
    }, FILE_POLL_INTERVAL)
    return () => {
      offBoard()
      offFs()
      window.clearInterval(poll)
    }
  }, [scope, filePath, session])

  // Flush when the person leaves the tab or the window.
  useEffect(() => {
    const flush = (): void => void session.flush().catch((err: unknown) => console.warn('[board-file] flush failed:', err))
    const onVis = (): void => {
      if (document.visibilityState === 'hidden') flush()
    }
    window.addEventListener('blur', flush)
    document.addEventListener('visibilitychange', onVis)
    return () => {
      window.removeEventListener('blur', flush)
      document.removeEventListener('visibilitychange', onVis)
    }
  }, [session])

  // The name's rename: the file tree's route, after what's queued is saved (a save after the rename would find the
  // file gone). The tab follows the file to its new path; the pane remounts there.
  const rename = async (fileName: string): Promise<void> => {
    await session.flush().catch(() => undefined)
    const v = session.view()
    if (v.queued + v.held.filter((h) => h.heldId === null).length > 0) throw new Error('Some edits aren’t saved yet. Save them, then rename.')
    const r = await daemonCliPost<{ path: string }>(scope, 'fs/rename', { old_path: filePath, new_name: fileName })
    fileUndoFor(scope.hostKey).push({ type: 'rename', oldPath: filePath, newPath: r.path })
    if (tabId) room.tabs.getState().retargetUntitledFile(tabId, filePath, r.path)
  }

  const close = async (): Promise<void> => {
    if (!tabId) return
    if (await askBeforeBoardFileLoss('close', [session])) closeTab(tabId)
  }

  const p = view.phase
  if (p.kind === 'loading') return <Card title="Opening the board…" />
  if (p.kind === 'empty') {
    return (
      <Card title="This file is empty">
        <CardButton
          testid="board-file-start"
          onClick={() =>
            void session.startBoard().catch((err: unknown) => useToastStore.getState().addToast(`K2 couldn’t start a board here: ${err instanceof Error ? err.message : String(err)}`, 'error'))
          }
        >
          Start a board here
        </CardButton>
      </Card>
    )
  }
  if (p.kind === 'large') {
    return (
      <Card title={`This board is large (${fmtBytes(p.bytes)})`}>
        <div>Opening it can take a while and uses a lot of memory. Nothing is cut.</div>
        <CardButton testid="board-file-open-anyway" onClick={() => void session.openAnyway()}>
          Open anyway
        </CardButton>
      </Card>
    )
  }
  if (p.kind === 'newer') return <Card title={`Made by a newer K2 (needs reader ${p.needsReader ?? '?'}, this is ${p.ours ?? '?'})`}><div>Update K2 to open it. The file wasn’t changed.</div><CardButton onClick={() => void close()}>Close</CardButton></Card>
  if (p.kind === 'shape') return <Card title="This server’s K2 is newer than this app"><div>Update K2 to open this board. The file wasn’t changed.</div><CardButton onClick={() => void close()}>Close</CardButton></Card>
  if (p.kind === 'corrupt') {
    return (
      <Card title="K2 can’t read this board">
        <div>{p.hint}</div>
        <div className="font-mono" data-testid="board-file-code">
          {p.code}
          {p.at ? ` (${p.at})` : ''}
        </div>
        {p.badIds.length > 0 ? (
          <div data-testid="board-file-bad-ids">
            {(p.badIdsTotal ?? p.badIds.length)} bad ids: {p.badIds.slice(0, 5).join(', ')}
            {p.badIdsTruncated || p.badIds.length > 5 ? '…' : ''}
          </div>
        ) : null}
        <div>The file wasn’t changed.</div>
        <CardButton onClick={() => void close()}>Close</CardButton>
      </Card>
    )
  }
  if (p.kind === 'off') return <BoardFileCard filePath={filePath} />
  if (p.kind === 'gone') return <Card title="This file was moved or deleted"><CardButton onClick={() => void close()}>Close</CardButton></Card>
  if (p.kind === 'denied') return <Card title="You can’t open this board"><div>{p.hint}</div><CardButton onClick={() => void close()}>Close</CardButton></Card>
  if (p.kind === 'unreachable') return <Card title="K2 can’t reach this board right now"><div>{p.hint}</div><CardButton onClick={() => void session.start()}>Try again</CardButton></Card>

  return (
    <div ref={boxRef} className="relative flex h-full min-h-0 flex-col" data-board-file-pane={filePath}>
      <BoardFileBanners view={view} session={session} filePath={filePath} onClose={() => void close()} />
      <BoardFileFrame key={filePath} session={session} filePath={filePath} tabId={tabId ?? filePath} api={api} rename={rename} />
      <HeldChip held={view.held} session={session} />
    </div>
  )
}

function BoardFileBanners({ view, session, filePath, onClose }: { view: BoardFileView; session: BoardFileSession; filePath: string; onClose(): void }): React.JSX.Element | null {
  const [copy, setCopy] = useState<{ to: string; missing: string[] | null; busy: boolean; error: string | null } | null>(null)
  const startCopy = (): void => setCopy({ to: `${dirName(filePath)}/${baseName(filePath).replace(/\.k5board$/i, '')} copy.k5board`, missing: null, busy: false, error: null })
  const runCopy = async (dropMissing: boolean): Promise<void> => {
    if (!copy) return
    setCopy({ ...copy, busy: true, error: null })
    try {
      const r = await session.saveCopy(copy.to, dropMissing ? { dropMissing: true } : undefined)
      if (r.ok) {
        setCopy(null)
        useToastStore.getState().addToast(`Saved a copy: ${r.name}${r.missing.length ? ` (without ${r.missing.length} pictures)` : ''}`, 'success')
      } else {
        setCopy({ ...copy, busy: false, missing: r.missing })
      }
    } catch (err) {
      setCopy({ ...copy, busy: false, error: err instanceof Error ? err.message : String(err) })
    }
  }
  const lines: { key: string; text: string; actions?: React.ReactNode }[] = []
  const copyBtn = <CardButton testid="board-file-save-copy" onClick={startCopy}>Save a copy</CardButton>
  if (view.file === 'moved') lines.push({ key: 'moved', text: view.copiedTo ? `This file was moved or deleted. Your board is in ${view.copiedTo}.` : 'This file was moved or deleted. Your edits are kept here until you close this tab.', actions: <>{copyBtn}<CardButton onClick={onClose}>Close</CardButton></> })
  if (view.file === 'unreadable') lines.push({ key: 'unreadable', text: 'K2 can’t read this file right now. Your edits are kept here while it tries again.' })
  if (view.file === 'unreadable-long') lines.push({ key: 'unreadable', text: 'K2 still can’t read this file. Your edits are kept here.', actions: copyBtn })
  if (view.file === 'ok' && view.status.state === 'error' && view.queued > 0) lines.push({ key: 'save', text: `${view.status.text}. Your edits are kept here.`, actions: copyBtn })
  if (view.file === 'ok' && view.readOnlyWhy && view.missing.length === 0) lines.push({ key: 'ro', text: `Read only: ${view.readOnlyWhy}.`, actions: copyBtn })
  if (view.missing.length > 0) {
    const n = view.missing.length
    lines.push({
      key: 'missing',
      text: n === 1 ? 'A picture is missing from this file.' : `${n} pictures are missing from this file.`,
      actions: view.missingBlocksSave ? (
        <>
          <CardButton testid="board-file-clear-missing" onClick={() => session.clearMissing()}>{n === 1 ? 'Clear it and keep saving' : `Clear ${n} and keep saving`}</CardButton>
          {copyBtn}
        </>
      ) : undefined,
    })
  }
  if (view.lostNote) {
    const n = view.lostNote.edits
    lines.push({
      key: 'lost',
      text: `K2 closed with ${n === 1 ? '1 edit' : `${n} edits`} to this board that hadn’t been saved. ${n === 1 ? 'It’s' : 'They’re'} gone.`,
      actions: <CardButton testid="board-file-lost-ok" onClick={() => session.setLostNote(null)}>OK</CardButton>,
    })
  }
  if (lines.length === 0 && !copy) return null
  return (
    <div className="flex flex-col border-b border-[var(--color-border)] text-xs text-[var(--color-text-primary)]" data-board-file-banners="">
      {lines.map((l) => (
        <div key={l.key} data-board-file-banner={l.key} className="flex items-center gap-2 px-3 py-1">
          <span className="min-w-0 flex-1">{l.text}</span>
          {l.actions}
        </div>
      ))}
      {copy ? (
        <div className="flex items-center gap-2 px-3 py-1" data-board-file-copy="">
          <span>Save to</span>
          <input data-testid="board-file-copy-to" className="min-w-0 flex-1 border border-[var(--color-border)] bg-transparent px-1 font-mono" value={copy.to} onChange={(e) => setCopy({ ...copy, to: e.target.value })} spellCheck={false} />
          {copy.missing ? (
            <>
              <span data-testid="board-file-copy-missing">{copy.missing.length === 1 ? '1 picture is gone.' : `${copy.missing.length} pictures are gone.`}</span>
              <CardButton testid="board-file-copy-without" onClick={() => void runCopy(true)}>{copy.missing.length === 1 ? 'Save without it' : `Save without ${copy.missing.length} pictures`}</CardButton>
            </>
          ) : (
            <CardButton testid="board-file-copy-go" onClick={() => void runCopy(false)}>{copy.busy ? 'Saving…' : 'Save'}</CardButton>
          )}
          <CardButton onClick={() => setCopy(null)}>Cancel</CardButton>
          {copy.error ? <span data-testid="board-file-copy-error">{copy.error}</span> : null}
        </div>
      ) : null}
    </div>
  )
}

/** BT-25: nothing resolves itself. The chip says the edit isn't saved; its list shows mine and theirs, field by field. */
function HeldChip({ held, session }: { held: HeldGroup[]; session: BoardFileSession }): React.JSX.Element | null {
  const [open, setOpen] = useState(false)
  if (held.length === 0) return null
  const n = held.length
  const why = held.some((h) => h.objects.some((o) => o.why === 'deleted')) ? 'someone deleted this' : 'someone changed this'
  return (
    <div className="absolute bottom-3 left-1/2 z-10 max-h-[50%] w-[min(480px,90%)] -translate-x-1/2 overflow-auto border border-[var(--color-border)] bg-[var(--color-bg-elevated)] text-xs text-[var(--color-text-primary)]" data-board-file-held="">
      <button type="button" className="w-full cursor-pointer px-3 py-2 text-left" data-testid="board-file-held-chip" onClick={() => setOpen(!open)}>
        {n === 1 ? `1 change didn’t save: ${why}.` : `${n} changes didn’t save.`} Not in the file until you choose.
      </button>
      {open
        ? held.map((h) => (
            <div key={h.id} className="border-t border-[var(--color-border)] px-3 py-2" data-board-file-held-group={h.id}>
              {h.objects.map((o) => (
                <div key={o.id}>
                  <button type="button" className="cursor-pointer underline" onClick={() => session.focusHeld(o.id)}>Show it on the board</button>
                  {o.why === 'deleted' ? <div>Someone deleted this item.</div> : o.why === 'exists' ? <div>An item with this id is already in the file.</div> : null}
                  {o.fields.length > 0 ? (
                    <table className="my-1 w-full table-fixed" data-testid="board-file-held-fields">
                      <thead><tr><th className="text-left font-normal text-[var(--color-text-muted)]">Field</th><th className="text-left font-normal text-[var(--color-text-muted)]">Mine</th><th className="text-left font-normal text-[var(--color-text-muted)]">Theirs</th></tr></thead>
                      <tbody>
                        {o.fields.map((f) => (
                          <tr key={f.field}><td className="truncate">{f.field}</td><td className="truncate" title={f.mine}>{f.mine}</td><td className="truncate" title={f.theirs}>{f.theirs}</td></tr>
                        ))}
                      </tbody>
                    </table>
                  ) : null}
                </div>
              ))}
              <div className="mt-1 flex gap-2">
                {h.objects.some((o) => o.why === 'deleted' && !o.mine) ? (
                  <span data-testid="board-file-no-put-back">K2 no longer has the whole item, so it can’t be put back. Your change is listed above.</span>
                ) : (
                  <CardButton testid="board-file-use-mine" onClick={() => session.useMine(h.id)}>{h.objects.some((o) => o.why === 'deleted') ? 'Put it back' : 'Use mine'}</CardButton>
                )}
                <CardButton testid="board-file-use-theirs" onClick={() => session.useTheirs(h.id)}>{h.objects.some((o) => o.why === 'deleted') ? 'Let it go' : 'Use theirs'}</CardButton>
              </div>
            </div>
          ))
        : null}
    </div>
  )
}

/** The inner bridge a Garden gives its widgets: a tab reaches no agents and no Thread. */
const noInnerBridge = (): ZenWidgetBridge => {
  const refuse = (): never => {
    throw new ZenCustomRefusal('verb_unavailable', 'That isn’t available in a board file.')
  }
  return new Proxy({}, { get: () => refuse }) as ZenWidgetBridge
}

type Loaded = { html: string; nonce: string; prelude: string; hash: string }

/** The frame hides its own name and starts its bar after K2's (BoardFileName) by these two. */
function nameVars(width: number): Record<string, string> {
  return width > 0 ? { '--zen-board-name-host': 'none', '--zen-board-name-w': `${width + 8}px` } : {}
}

function BoardFileFrame({ session, filePath, tabId, api, rename }: { session: BoardFileSession; filePath: string; tabId: string; api: BoardFileApi; rename(fileName: string): Promise<void> }): React.JSX.Element {
  const [loaded, setLoaded] = useState<Loaded | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const [reloads, setReloads] = useState(0)
  const [linkAsk, setLinkAsk] = useState<{ ask: ZenLinkAsk; resolve: (a: ZenLinkAnswer) => void } | null>(null)
  const [downloadAsk, setDownloadAsk] = useState<{ ask: ZenDownloadAsk; resolve: (a: ZenDownloadAnswer) => void } | null>(null)
  const boxRef = useRef<HTMLDivElement | null>(null)
  const frameRef = useRef<HTMLIFrameElement | null>(null)
  const hostRef = useRef<{ dispose(): void } | null>(null)
  const gestureKey = `board-file:${tabId}`
  const label = 'Board'
  // The name's width reaches the frame as theme variables: a width change is a theme change to it.
  const nameWidth = useRef(0)
  const themeListeners = useRef(new Set<() => void>())
  const onNameWidth = useCallback((px: number) => {
    if (px === nameWidth.current) return
    nameWidth.current = px
    for (const cb of themeListeners.current) cb()
  }, [])
  const frameTheme = (): ReturnType<typeof boardFileTheme> => {
    const t = boardFileTheme(boxRef.current)
    return { ...t, vars: { ...t.vars, ...nameVars(nameWidth.current) } }
  }

  useEffect(() => {
    let live = true
    void Promise.all([fetchZenWidgetBundle(BOARD_FILE_WIDGET), zenLib.texts(['k2-png@1'])])
      .then(([bundle, libs]) => {
        if (!live) return
        const prelude = zenFramePrelude({ nonce: bundle.nonce, theme: frameTheme(), libs })
        setLoaded({ html: bundle.html, nonce: bundle.nonce, prelude, hash: bundle.hash })
      })
      .catch((err: unknown) => {
        if (live) setProblem(zenWidgetError(err).message)
      })
    return () => {
      live = false
    }
  }, [reloads])

  useEffect(() => () => hostRef.current?.dispose(), [])
  useEffect(() => {
    const el = frameRef.current
    if (!loaded || !el) return undefined
    return registerZenCanvasFrame(gestureKey, el)
  }, [loaded, gestureKey])

  const onLoad = (el: HTMLIFrameElement): void => {
    const win = el.contentWindow
    if (!win) return
    hostRef.current?.dispose()
    const widget: ZenCustomWidgetPayload = {
      id: gestureKey,
      kind: 'custom',
      widget: BOARD_FILE_WIDGET,
      column: 0,
      props: { config: {} },
      name: label,
      hash: loaded?.hash ?? '',
      caps: ['store:read', 'store:write', 'links:open', 'downloads:save', 'uploads:pick'],
      requested: ['store:read', 'store:write', 'links:open', 'downloads:save', 'uploads:pick'],
      paused: null,
    } as unknown as ZenCustomWidgetPayload
    const theme = boardFileTheme(boxRef.current)
    const hello: ZenFrameHello = {
      k2: 'hello',
      v: 1,
      caps: [...widget.caps],
      features: [...BOARD_FILE_FEATURES],
      widget: { id: gestureKey, name: label, garden: boardFileConsentKey },
      config: {},
      motion: { reduced: theme.reduced },
    }
    const join = createBoardFilePartJoiner()
    hostRef.current = startZenFrameHost(win, {
      hello,
      label: `${BOARD_FILE_WIDGET} (${baseName(filePath)})`,
      // K2 keeps the edits outside the frame: a new frame comes back with them.
      stop: (reason) => {
        console.warn('[board-file] the frame stopped:', reason)
        session.hold(false) // whatever it was typing went with it
        hostRef.current = null
        setLoaded(null)
        setReloads((n) => n + 1)
      },
      focused: () => zenFrameHasFocus(frameRef.current),
      onChord: () => undefined, // Zen's keys don't mean anything in a tab.
      onPointer: (down) => zenCanvasPointer(gestureKey, down),
      makeLayer: (push, pushError) => {
        const blobs = createBoardFileBlobs(api, filePath, session)
        return createZenCustomLayer({
          widget: () => widget,
          gardenId: () => boardFileConsentKey,
          inner: noInnerBridge(),
          boundRows: () => [],
          ensureConversation: async () => undefined,
          push,
          pushError,
          blobs,
          boardFile: {
            open: () => session.open(),
            submit: (p) => {
              const text = join(p)
              if (text === null) return
              session.submit({ gesture: p.gesture, kind: p.kind, byAll: p.byAll, ops: JSON.parse(text) as BoardOp[] })
            },
            hold: (on) => session.hold(on),
            flush: () => session.flush(),
            key: (k) => redispatchBoardFileKey(k),
            events: (fn) => session.onEvent(fn),
          },
          uploads: {
            pressed: () => zenCanvasPressedWithin(gestureKey, ZEN_PICK_GESTURE_MS),
            pick: async (opts) => {
              try {
                // The file input: a board file's pictures go through its own routes, in bytes.
                return await zenPick(
                  {
                    web: () => true,
                    native: async () => [],
                    browser: (o) => pickBrowserFiles(o),
                    ingest: async () => [],
                    put: (bytes, o) => blobs.put(bytes, o),
                  },
                  opts,
                )
              } catch (err) {
                if (err instanceof ZenUploadsError) throw new ZenCustomRefusal(err.code, err.message)
                throw err
              }
            },
            onDrop: () => () => undefined,
          },
          downloads: {
            pressed: () => zenCanvasPressedWithin(gestureKey, ZEN_PICK_GESTURE_MS),
            consent: ({ name, size }) =>
              zenDownloadConsent(
                daemonZenDownloadConsent,
                boardFileConsentKey,
                BOARD_FILE_WIDGET,
                { name, size, widget: label, garden: 'board files' },
                (ask) => new Promise<ZenDownloadAnswer>((resolve) => setDownloadAsk({ ask, resolve })),
              ),
            spend: (size) => spendZenDownload(boardFileConsentKey, BOARD_FILE_WIDGET, size),
            save: (name, read) => downloadChunks(name, read, { quarantine: true }),
          },
          links: {
            pressed: () => zenCanvasPressedWithin(gestureKey, ZEN_PICK_GESTURE_MS),
            open: createZenLinks({
              gardenId: () => boardFileConsentKey,
              sites: fetchZenLinkSites,
              allow: allowZenLinkSite,
              ask: (ask) => new Promise<ZenLinkAnswer>((resolve) => setLinkAsk({ ask, resolve })),
              describe: () => ({ widget: label, garden: 'board files' }),
              openUrl: (url) => openUrl(url),
            }).open,
          },
          stop: () => undefined,
          gesture: () => zenCanvasPressedWithin(gestureKey, ZEN_PICK_GESTURE_MS) || zenFrameHasFocus(frameRef.current),
          paused: () => false,
          pauseForRunaway: () => undefined,
          theme: () => {
            const t = frameTheme()
            return { theme: { scheme: t.scheme, vars: t.vars }, chrome: { corners: null, stoplights: null }, motion: { reduced: t.reduced } }
          },
          onThemeChange: (cb) => {
            themeListeners.current.add(cb)
            const off = (): void => void themeListeners.current.delete(cb)
            if (typeof MutationObserver === 'undefined') return off
            const mo = new MutationObserver(cb)
            mo.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'style', 'data-theme'] })
            return () => {
              off()
              mo.disconnect()
            }
          },
          setTimer: (fn, ms) => window.setTimeout(fn, ms),
          clearTimer: (t) => window.clearTimeout(t as number),
          now: () => Date.now(),
        })
      },
    })
  }

  if (problem) return <Card title="K2 can’t load the board’s canvas"><div>{problem}</div></Card>
  return (
    <div ref={boxRef} className="relative flex min-h-0 min-w-0 flex-1" data-board-file-frame-slot="">
      {loaded ? (
        <HtmlFrame
          key={reloads}
          html={loaded.html}
          profile="widget"
          nonce={loaded.nonce}
          prelude={loaded.prelude}
          title={label}
          testId="board-file-frame"
          frameRef={(el) => {
            frameRef.current = el
          }}
          onLoad={onLoad}
          className="h-full w-full flex-1"
          style={{ border: 0, background: 'transparent', minHeight: 0 }}
        />
      ) : null}
      {loaded ? <BoardFileName filePath={filePath} rename={rename} onWidth={onNameWidth} /> : null}
      {downloadAsk ? <ZenDownloadSheet ask={downloadAsk.ask} onDone={(a) => { downloadAsk.resolve(a); setDownloadAsk(null) }} /> : null}
      {linkAsk ? <ZenLinkSheet ask={linkAsk.ask} onDone={(a) => { linkAsk.resolve(a); setLinkAsk(null) }} /> : null}
    </div>
  )
}
