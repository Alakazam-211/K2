// Composer compose bar docked beneath a terminal pane. The human types a
// draft (persisted per-session in the THIN CLIENT — renderer localStorage,
// per-user/per-device, NEVER the daemon which may be remote/shared — so it
// survives workspace/tab switches AND app crashes/restarts), hits
// Enter, and the message is delivered to the agent via the daemon route
// POST /cli/terminal/send-message (attributed `[from <name>] `, submitted
// once through the per-session injection lock). Esc / Ctrl+C inject the
// same PTY bytes as the terminal (cancel the current turn) without
// stealing compose focus. Other raw TUI control (arrows, menu nav)
// still goes through the terminal itself. Renderer-hide is gated by
// composerPermitted (1c); the daemon enforces the same gate server-side.
//
// File drops: local paths are inserted into the draft; on a remote host the
// same `.k2/downloads` upload path as terminal drops runs first, then the
// host path is inserted. Window-level routing is external-drop-router
// (`[data-compose-bar]`); HTML5 File drops also land here via onDrop.
//
// Condensed UI: just the input + its placeholder hint — no title, no send
// button, no status lane, no collapse control. A successful send clears the
// box; a failed send restores the text (the box reappearing IS the feedback).

import { useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import {
  buildComposeDropPayload,
  filesFromDataTransfer,
} from '@/lib/external-drop-router'
import { executeBrowserFileDrop, executeRemoteDrop } from '@/lib/handle-remote-drop'
import {
  composeAttachPlan,
  pickLocalComposeFiles,
  pickRemoteComposeFile,
} from '@/lib/pick-compose-files'
import { useConnectHostStore } from '@/stores/connect-host'
import { useToastStore } from '@/stores/toast'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { isEffectivelyHidden } from '@/lib/workspace-switch-focus'
import {
  type ComposeHistoryItem,
  type ComposeSlashCommand,
  type MsgResponse,
  applyComposeHistoryNav,
  clearComposeCaret,
  COMPOSE_SLASH_COMMANDS,
  COMPOSE_TEXTAREA_MAX_HEIGHT,
  composeCanSend,
  composeHistoryKeyAction,
  composeInterruptSequence,
  composeSlashMenuKeyAction,
  composeSlashMenuOpenFromDraft,
  composeSlashSpaceCommit,
  composeSlashTypeaheadQuery,
  composeTextareaHeight,
  consumeComposeSlashToken,
  composeAgentNameFromProjects,
  composeMessagePlaceholder,
  composerPermitted,
  extractImagePathsFromDraft,
  filterComposeSlashCommands,
  mapMsgResponseToStatus,
  readComposeCaret,
  removePathFromDraft,
  shouldSendOnKey,
  writeComposeCaret,
} from './terminalCompose'
import { useSessionViewChrome } from '@/components/SessionView/sessionViewChrome'
import { loadHostImageObjectUrl, revokeObjectUrl } from '@/lib/load-host-binary'

interface TerminalComposeBarProps {
  /** Resolved PTY SessionId for this pane — the pane's `terminalId`. */
  sessionId: string
  /**
   * Workspace cwd for this terminal — remote drops upload into
   * `<cwd>/.k2/downloads/` (same as terminal grid drops).
   */
  workspacePath?: string
  /**
   * Inject raw PTY bytes into this pane's session (same path as
   * typing in the grid). Used for Esc / Ctrl+C turn-cancel.
   */
  onInjectInput?: (data: string) => void
  /**
   * When set, this bar always sends here (split view: one bar under
   * the PTY, one under Thread). Otherwise the session-view tab picks.
   */
  sendDestination?: 'pty' | 'thread'
}

/** Insert `chunk` into draft at caret (or append). Adds a separating space when needed. */
export function insertIntoDraft(draft: string, chunk: string, caret: number | null): string {
  const piece = chunk.trimEnd() + (chunk.endsWith(' ') ? ' ' : chunk.length > 0 ? ' ' : '')
  if (!piece.trim()) return draft
  if (caret === null || caret < 0 || caret > draft.length) {
    if (!draft) return piece
    const needSpace = !/\s$/.test(draft) && !/^\s/.test(piece)
    return draft + (needSpace ? ' ' : '') + piece
  }
  const before = draft.slice(0, caret)
  const after = draft.slice(caret)
  const needBefore = before.length > 0 && !/\s$/.test(before) && !/^\s/.test(piece)
  const mid = (needBefore ? ' ' : '') + piece
  return before + mid + after
}

export function TerminalComposeBar({
  sessionId,
  workspacePath = '',
  onInjectInput,
  sendDestination,
}: TerminalComposeBarProps): React.JSX.Element | null {
  // 1c (D4) + #67 renderer-hide: shown iff the active host is LOCAL (owner)
  // OR the app-level master is on OR the ACTIVE WORKSPACE opted into remote
  // instruction (default OFF). The daemon enforces the same gate per-workspace
  // server-side; this hide is defense-in-depth only.
  const isLocalHost = useConnectHostStore((s) => s.activeHost === 'local')
  const allowRemoteInstruct = useSettingsStore((s) => s.allowRemoteInstruct)
  // Per-workspace opt-in for the currently-active workspace. Approximate on
  // the renderer (the daemon resolves the EXACT target session's workspace);
  // good enough for the convenience hide.
  const perWorkspaceAllow = useProjectsStore((s) => {
    const active = s.projects.find((p) => p.id === s.activeProjectId)
    return (active?.allowRemoteInstruct ?? 0) === 1
  })
  const permitted = composerPermitted({ isLocalHost, allowRemoteInstruct, perWorkspaceAllow })
  // Match Code Editor → Appearance → Font Size (default 12).
  const editorFontSize = useSettingsStore((s) => s.editor.fontSize) || 12
  const agentName = useProjectsStore((s) =>
    composeAgentNameFromProjects(s.projects, workspacePath),
  )
  const messagePlaceholder = composeMessagePlaceholder(agentName)
  const sessionChrome = useSessionViewChrome()
  const sendOnThread =
    sendDestination === 'thread' ||
    (sendDestination !== 'pty' && sessionChrome?.viewTab === 'thread')
  const threadAddr = sessionChrome?.overlayAddr ?? ''

  // Draft persistence (thin client): key the draft by this pane's PTY session
  // and back it with localStorage so switching workspaces/tabs restores each
  // composer's own text instead of clearing it, and a crash/restart never loses
  // it. Per-user/per-device by nature (localStorage is the desktop app's own
  // storage); the draft never touches the daemon.
  const draftKey = sendOnThread
    ? `k2:composer:draft:${sessionId}:thread`
    : `k2:composer:draft:${sessionId}`
  const [draft, setDraft] = useState<string>(() => {
    try {
      return localStorage.getItem(`k2:composer:draft:${sessionId}`) ?? ''
    } catch {
      return ''
    }
  })
  const [sending, setSending] = useState(false)
  const [history, setHistory] = useState<string[]>([])
  const [historyIndex, setHistoryIndex] = useState(-1)
  const [slashCommand, setSlashCommand] = useState<ComposeSlashCommand | null>(null)
  const [slashMenuOpen, setSlashMenuOpen] = useState(false)
  const [slashMenuFromTypeahead, setSlashMenuFromTypeahead] = useState(false)
  const [slashHighlight, setSlashHighlight] = useState(0)
  const [slashMenuPos, setSlashMenuPos] = useState<{ left: number; bottom: number } | null>(null)
  const historyDraftRef = useRef('')
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const barRef = useRef<HTMLDivElement>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const slashBtnRef = useRef<HTMLButtonElement>(null)
  const slashMenuRef = useRef<HTMLDivElement>(null)
  const imagePaths = extractImagePathsFromDraft(draft)

  // Reload the saved draft when the pane's session changes — this component can
  // be reused with a new sessionId on a workspace/tab switch without remounting.
  useEffect(() => {
    if (!sessionId) {
      setDraft('')
      setHistoryIndex(-1)
      historyDraftRef.current = ''
      setSlashCommand(null)
      setSlashMenuOpen(false)
      setSlashMenuFromTypeahead(false)
      setSlashHighlight(0)
      return
    }
    try {
      setDraft(localStorage.getItem(draftKey) ?? '')
    } catch {
      setDraft('')
    }
    setHistoryIndex(-1)
    historyDraftRef.current = ''
    setSlashCommand(null)
    setSlashMenuOpen(false)
    setSlashMenuFromTypeahead(false)
    setSlashHighlight(0)
  }, [draftKey, sessionId])

  // Persist on every change (localStorage writes are cheap + synchronous — this
  // is the crash-durable store). An empty draft clears the key.
  useEffect(() => {
    if (!sessionId) return
    try {
      if (draft) localStorage.setItem(draftKey, draft)
      else {
        localStorage.removeItem(draftKey)
        clearComposeCaret(sessionId)
      }
    } catch {
      /* storage disabled/full — draft just won't persist */
    }
  }, [draft, draftKey, sessionId])

  // Auto-grow with real draft text only. Empty boxes stay one line —
  // the long placeholder used to wrap (especially before width settled
  // or on a hidden tab) and pin every bar at the 160px cap until typed.
  const autoGrow = useCallback(() => {
    const el = textareaRef.current
    if (!el || isEffectivelyHidden(el)) return
    if (!el.value) {
      el.style.height = `${composeTextareaHeight({
        value: '',
        scrollHeight: 0,
        fontSize: editorFontSize,
      })}px`
      return
    }
    el.style.height = 'auto'
    el.style.height = `${composeTextareaHeight({
      value: el.value,
      scrollHeight: el.scrollHeight,
      fontSize: editorFontSize,
    })}px`
  }, [editorFontSize])

  useEffect(() => {
    autoGrow()
  }, [draft, autoGrow])

  useEffect(() => {
    const el = textareaRef.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => autoGrow())
    ro.observe(el)
    return () => ro.disconnect()
  }, [autoGrow])

  const persistCaret = useCallback(() => {
    const el = textareaRef.current
    if (!el || !sessionId) return
    writeComposeCaret(sessionId, el.selectionStart, el.selectionEnd, el.value.length)
  }, [sessionId])

  const restoreCaret = useCallback(() => {
    const el = textareaRef.current
    if (!el) return
    const { start, end } = readComposeCaret(sessionId, el.value.length)
    el.setSelectionRange(start, end)
  }, [sessionId])

  // Workspace-switch "Message agent" pref: take the caret when this bar
  // mounts (or its session changes) so we win the race against the
  // terminal-grid remount. Terminal-mode users are left alone.
  useEffect(() => {
    if (useSettingsStore.getState().workspaceSwitchFocus !== 'composer') return
    const id = requestAnimationFrame(() => {
      const el = textareaRef.current
      // Hidden tabs stay mounted (`display:none` + aria-hidden). Focusing
      // those sends typing to the wrong session.
      if (!el || isEffectivelyHidden(el)) return
      el.focus()
      restoreCaret()
    })
    return () => cancelAnimationFrame(id)
  }, [sessionId, restoreCaret])

  // Workspace-shared send history (daemon). Drafts stay localStorage.
  useEffect(() => {
    setHistoryIndex(-1)
    historyDraftRef.current = ''
    if (!workspacePath) {
      setHistory([])
      return
    }
    let cancelled = false
    void daemonCliGet<{ items?: ComposeHistoryItem[] }>('terminal/compose-history', {
      workspace_path: workspacePath,
    })
      .then((resp) => {
        if (cancelled) return
        const bodies = (resp.items ?? [])
          .map((item) => item.body)
          .filter((body) => typeof body === 'string' && body.length > 0)
        setHistory(bodies)
      })
      .catch(() => {
        if (!cancelled) setHistory([])
      })
    return () => {
      cancelled = true
    }
  }, [workspacePath])

  const insertPathsText = useCallback((payload: string) => {
    if (!payload) return
    const el = textareaRef.current
    const caret = el ? el.selectionStart : null
    setDraft((cur) => {
      const next = insertIntoDraft(cur, payload, caret)
      // Restore caret after React re-render.
      requestAnimationFrame(() => {
        const ta = textareaRef.current
        if (!ta) return
        const pos = Math.min(
          (caret ?? cur.length) + (next.length - cur.length),
          next.length,
        )
        ta.focus()
        ta.setSelectionRange(pos, pos)
      })
      return next
    })
  }, [])

  // Window-level external-drop-router / internal file-drag → insert event.
  useEffect(() => {
    const el = barRef.current
    if (!el) return
    const onInsert = (e: Event) => {
      const data = (e as CustomEvent<{ data: string }>).detail?.data
      if (data) insertPathsText(data)
    }
    el.addEventListener('k2so:compose-insert', onInsert)
    return () => el.removeEventListener('k2so:compose-insert', onInsert)
  }, [insertPathsText])

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault()
    e.stopPropagation()
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy'
  }, [])

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault()
      e.stopPropagation()
      const files = e.dataTransfer.files
      if (files && files.length > 0) {
        const paths: string[] = []
        for (let i = 0; i < files.length; i++) {
          const p = (files[i] as unknown as { path?: string }).path
          if (p) paths.push(p)
        }
        if (paths.length > 0) {
          if (useConnectHostStore.getState().activeHost !== 'local') {
            void executeRemoteDrop(
              paths,
              { kind: 'terminal' },
              { workspacePath: workspacePath || undefined },
              buildComposeDropPayload,
            ).then((payload) => {
              if (payload) insertPathsText(payload)
            })
          } else {
            insertPathsText(buildComposeDropPayload(paths))
          }
          return
        }
        // Hosted web / no File.path — upload File bytes then insert host path.
        const browserFiles = filesFromDataTransfer(e.dataTransfer)
        if (browserFiles.length > 0) {
          void executeBrowserFileDrop(
            browserFiles,
            { kind: 'terminal' },
            { workspacePath: workspacePath || undefined },
            buildComposeDropPayload,
          ).then((payload) => {
            if (payload) insertPathsText(payload)
          })
          return
        }
      }
      // Internal path list (text/plain from some drag sources)
      const text = e.dataTransfer.getData('text/plain')
      if (text?.trim()) insertPathsText(text.trim() + ' ')
    },
    [insertPathsText, workspacePath],
  )

  const handleAttachClick = useCallback(() => {
    const plan = composeAttachPlan()
    if (plan.kind === 'native') {
      void pickLocalComposeFiles()
        .then((paths) => {
          if (paths && paths.length > 0) insertPathsText(buildComposeDropPayload(paths))
        })
        .catch((err) => {
          useToastStore.getState().addToast(
            `Couldn't attach: ${err instanceof Error ? err.message : String(err)}`,
            'error',
          )
        })
      return
    }
    if (plan.kind === 'web-input') {
      fileInputRef.current?.click()
      return
    }
    void pickRemoteComposeFile().then((path) => {
      if (path) insertPathsText(buildComposeDropPayload([path]))
    })
  }, [insertPathsText])

  const handleLocalFiles = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const list = e.target.files
      e.target.value = ''
      if (!list || list.length === 0) return
      const paths: string[] = []
      for (let i = 0; i < list.length; i++) {
        const p = (list[i] as unknown as { path?: string }).path
        if (p) paths.push(p)
      }
      if (paths.length > 0) {
        insertPathsText(buildComposeDropPayload(paths))
        return
      }
      const browserFiles = Array.from(list)
      if (browserFiles.length > 0) {
        void executeBrowserFileDrop(
          browserFiles,
          { kind: 'terminal' },
          { workspacePath: workspacePath || undefined },
          buildComposeDropPayload,
        ).then((payload) => {
          if (payload) insertPathsText(payload)
        })
      }
    },
    [insertPathsText, workspacePath],
  )

  const send = useCallback(async () => {
    const text = draft.trim()
    const command = slashCommand
    if (!composeCanSend({ draft, sending, command }) || sending) return
    if (sendOnThread) {
      if (!threadAddr.trim()) return
    } else if (!sessionId) {
      return
    }

    setSending(true)
    setDraft('') // optimistic clear (PRD 1b); restored below only on failure
    setSlashMenuOpen(false)
    setSlashMenuFromTypeahead(false)

    try {
      if (sendOnThread) {
        const body: { addr: string; text: string; via: string; command?: string } = {
          addr: threadAddr,
          text,
          via: 'compose',
        }
        if (command) body.command = command
        const resp = await daemonCliPost<{ ok?: boolean }>('thread/post', body)
        if (resp?.ok === false) {
          setDraft((cur) => (cur.length === 0 ? text : cur))
        } else {
          if (text) setHistory((prev) => [text, ...prev].slice(0, 50))
          setHistoryIndex(-1)
          historyDraftRef.current = ''
          setSlashCommand(null)
        }
      } else {
        const body: { session_id: string; text: string; command?: string } = {
          session_id: sessionId,
          text,
        }
        if (command) body.command = command
        const resp = await daemonCliPost<MsgResponse>('terminal/send-message', body)
        // Failed send → restore the text so it's not lost (the box reappearing
        // IS the feedback) — but never clobber a fresh draft already started.
        // Keep the selected slash-command on failure so retry still sends it.
        if (mapMsgResponseToStatus(resp).kind !== 'delivered') {
          setDraft((cur) => (cur.length === 0 ? text : cur))
        } else {
          if (text) setHistory((prev) => [text, ...prev].slice(0, 50))
          setHistoryIndex(-1)
          historyDraftRef.current = ''
          setSlashCommand(null)
        }
      }
    } catch {
      setDraft((cur) => (cur.length === 0 ? text : cur))
    } finally {
      setSending(false)
    }
  }, [draft, sending, sessionId, sendOnThread, threadAddr, slashCommand])

  const slashTypeaheadQuery = composeSlashTypeaheadQuery(draft)
  const slashMatches =
    slashMenuFromTypeahead && slashTypeaheadQuery != null
      ? filterComposeSlashCommands(slashTypeaheadQuery)
      : COMPOSE_SLASH_COMMANDS
  const slashMatchKey = slashMatches.map((item) => item.command).join(',')

  const placeSlashMenu = useCallback(() => {
    const rect = slashBtnRef.current?.getBoundingClientRect()
    if (!rect) return
    const width = 200
    const left = Math.max(4, Math.min(rect.left, window.innerWidth - width - 4))
    setSlashMenuPos({
      left,
      bottom: window.innerHeight - rect.top + 4,
    })
  }, [])

  const closeSlashMenu = useCallback(() => {
    setSlashMenuOpen(false)
    setSlashMenuFromTypeahead(false)
  }, [])

  const selectSlashCommand = useCallback(
    (cmd: ComposeSlashCommand, opts?: { toggle?: boolean }) => {
      const nextCmd = opts?.toggle && slashCommand === cmd ? null : cmd
      setSlashCommand(nextCmd)
      if (nextCmd != null && composeSlashMenuOpenFromDraft(draft)) {
        const nextDraft = consumeComposeSlashToken(draft)
        setDraft(nextDraft)
        requestAnimationFrame(() => {
          const ta = textareaRef.current
          if (!ta) return
          ta.focus()
          ta.setSelectionRange(nextDraft.length, nextDraft.length)
          writeComposeCaret(sessionId, nextDraft.length, nextDraft.length, nextDraft.length)
          autoGrow()
        })
      }
      closeSlashMenu()
    },
    [autoGrow, closeSlashMenu, draft, sessionId, slashCommand],
  )

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      // Menu path first: Enter selects (does not send); arrows move
      // highlight (do not walk compose send-history). 0 matches / menu
      // closed falls through to send then history.
      const menuAction = composeSlashMenuKeyAction({
        menuOpen: slashMenuOpen,
        matchCount: slashMatches.length,
        highlight: slashHighlight,
        key: e.key,
        shiftKey: e.shiftKey,
        isComposing: e.nativeEvent.isComposing,
      })
      if (menuAction) {
        e.preventDefault()
        e.stopPropagation()
        if (menuAction.kind === 'close') {
          closeSlashMenu()
        } else if (menuAction.kind === 'move') {
          setSlashHighlight(menuAction.highlight)
        } else {
          const cmd =
            slashMatches[slashHighlight]?.command ?? slashMatches[0]?.command
          if (cmd) selectSlashCommand(cmd)
        }
        return
      }
      const interrupt = composeInterruptSequence({
        key: e.key,
        ctrlKey: e.ctrlKey,
        metaKey: e.metaKey,
        altKey: e.altKey,
        isComposing: e.nativeEvent.isComposing,
      })
      if (interrupt) {
        e.preventDefault()
        e.stopPropagation()
        onInjectInput?.(interrupt)
        return
      }
      if (shouldSendOnKey({ key: e.key, shiftKey: e.shiftKey, isComposing: e.nativeEvent.isComposing })) {
        e.preventDefault()
        void send()
      } else {
        const histAction = composeHistoryKeyAction({
          key: e.key,
          selectionStart: e.currentTarget.selectionStart,
          selectionEnd: e.currentTarget.selectionEnd,
        })
        if (histAction) {
          const draftForRestore = historyIndex === -1 ? draft : historyDraftRef.current
          if (histAction === 'older' && historyIndex === -1) {
            historyDraftRef.current = draft
          }
          const next = applyComposeHistoryNav({
            action: histAction,
            index: historyIndex,
            draft: draftForRestore,
            items: history,
          })
          if (next.preventDefault) {
            e.preventDefault()
            setHistoryIndex(next.index)
            setDraft(next.text)
          }
        }
      }
      // Stop terminal-level / single-key shortcuts from firing while typing
      // (plain letters, arrows, etc.). Do NOT stop Cmd/Ctrl app chords —
      // useTerminalShortcuts lives on window bubble (Cmd+Shift+T launch
      // agent, Cmd+T tab, Cmd+W, Cmd+N, …) and was dead while this box
      // was focused. Project chat uses the same narrow pattern (Esc only).
      const isAppChord = e.metaKey || e.ctrlKey
      if (!isAppChord) {
        e.stopPropagation()
      }
    },
    [
      closeSlashMenu,
      draft,
      history,
      historyIndex,
      onInjectInput,
      selectSlashCommand,
      send,
      slashHighlight,
      slashMatches,
      slashMenuOpen,
    ],
  )

  const toggleSlashMenu = useCallback(() => {
    if (slashMenuOpen) {
      closeSlashMenu()
      return
    }
    placeSlashMenu()
    setSlashMenuFromTypeahead(false)
    setSlashHighlight(0)
    setSlashMenuOpen(true)
  }, [closeSlashMenu, placeSlashMenu, slashMenuOpen])

  useEffect(() => {
    setSlashHighlight(0)
  }, [slashMatchKey])

  useEffect(() => {
    if (!slashMenuOpen) return
    const onDown = (e: MouseEvent): void => {
      const t = e.target as Node
      if (slashBtnRef.current?.contains(t) || slashMenuRef.current?.contains(t)) return
      closeSlashMenu()
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      closeSlashMenu()
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey, true)
    }
  }, [closeSlashMenu, slashMenuOpen])

  // 1c renderer-hide (after all hooks): not permitted → render nothing.
  if (!permitted) return null

  // One condensed row — just the textarea.
  // min-w-0 on the flex row + field so app zoom (Cmd+=) reflows width
  // instead of pinning content-min-width and growing a horizontal scrollbar.
  const canSend = composeCanSend({
    draft,
    sending,
    command: slashCommand,
  })
  // Must match composeTextareaHeight: 4px pad × 2 + line-height 1.4.
  const firstLineH = Math.round(editorFontSize * 1.4 + 8)
  const btnSize = Math.min(firstLineH, Math.max(16, Math.round(editorFontSize * 1.4)))
  const iconSize = Math.max(10, Math.round(btnSize * 0.55))
  const btnNudge = Math.max(0, (firstLineH - btnSize) / 2)

  return (
    <div
      ref={barRef}
      className="flex min-w-0 w-full flex-shrink-0 flex-col gap-1.5 border-t border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-2 py-2"
      data-compose-bar=""
      data-session-id={sessionId}
      data-compose-destination={sendOnThread ? 'thread' : 'pty'}
      data-workspace-path={workspacePath || undefined}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      <input
        ref={fileInputRef}
        type="file"
        multiple
        className="sr-only"
        aria-hidden="true"
        tabIndex={-1}
        onChange={handleLocalFiles}
      />
      <div
        className="flex min-w-0 w-full flex-col bg-[var(--color-bg)]"
        style={{ border: '1px solid var(--color-border)', borderRadius: 0 }}
      >
      {imagePaths.length > 0 && (
        <div className="flex flex-wrap gap-1 px-1 pt-1" data-testid="compose-image-previews">
          {imagePaths.map((path) => (
            <ComposeImageThumb
              key={path}
              path={path}
              onRemove={() => setDraft((cur) => removePathFromDraft(cur, path))}
            />
          ))}
        </div>
      )}
      <div className="flex min-w-0 w-full items-end gap-1 px-1">
      <button
        type="button"
        aria-label="Attach file"
        title="Attach a file or image"
        onClick={handleAttachClick}
        className="inline-flex flex-shrink-0 items-center justify-center bg-[var(--color-bg-hover)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-border)] transition-colors"
        style={{
          borderRadius: 0,
          width: btnSize,
          height: btnSize,
          marginBottom: btnNudge,
        }}
      >
        <svg width={iconSize} height={iconSize} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
          <line x1="12" y1="5" x2="12" y2="19" />
          <line x1="5" y1="12" x2="19" y2="12" />
        </svg>
      </button>
      <button
          ref={slashBtnRef}
          type="button"
          aria-label="Slash command"
          title={slashCommand ? `${slashCommand} selected` : 'Slash command'}
          aria-haspopup="menu"
          aria-expanded={slashMenuOpen}
          aria-pressed={slashCommand != null}
          data-testid="compose-slash-button"
          disabled={sending}
          onClick={toggleSlashMenu}
          className={`inline-flex flex-shrink-0 items-center justify-center transition-colors disabled:opacity-40 disabled:cursor-not-allowed ${
            slashCommand
              ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90'
              : 'bg-[var(--color-bg-hover)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-border)]'
          }`}
          style={{
            borderRadius: 0,
            width: btnSize,
            height: btnSize,
            marginBottom: btnNudge,
            fontFamily:
              "'MesloLGM Nerd Font', 'MesloLGM Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace",
            fontSize: Math.max(11, iconSize),
            fontWeight: 600,
            lineHeight: 1,
          }}
        >
          /
        </button>
      {slashMenuOpen &&
        slashMenuPos &&
        slashMatches.length > 0 &&
        createPortal(
          <div
            ref={slashMenuRef}
            role="menu"
            data-testid="compose-slash-menu"
            className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)] py-0.5 shadow-lg"
            style={{
              position: 'fixed',
              left: slashMenuPos.left,
              bottom: slashMenuPos.bottom,
              zIndex: 99999,
              borderRadius: 0,
              minWidth: 180,
            }}
          >
            {slashMatches.map((item, index) => {
              const selected = slashCommand === item.command
              const highlighted =
                index === Math.min(slashHighlight, slashMatches.length - 1)
              return (
                <button
                  key={item.command}
                  type="button"
                  role="menuitem"
                  aria-checked={selected}
                  aria-selected={highlighted}
                  onClick={() =>
                    selectSlashCommand(item.command, { toggle: !slashMenuFromTypeahead })
                  }
                  className={`flex w-full items-baseline gap-2 px-2 py-1 text-left ${
                    highlighted
                      ? 'bg-[var(--color-bg-hover)]'
                      : 'hover:bg-[var(--color-bg-hover)]'
                  }`}
                  style={{ borderRadius: 0 }}
                >
                  <span
                    className="font-mono text-[11px]"
                    style={{
                      color:
                        selected || highlighted
                          ? 'var(--color-accent)'
                          : 'var(--color-text-primary)',
                    }}
                  >
                    {item.command}
                  </span>
                  <span className="text-[10px] text-[var(--color-text-muted)]">{item.title}</span>
                </button>
              )
            })}
          </div>,
          document.body,
        )}
      <textarea
        ref={textareaRef}
        value={draft}
        onChange={(e) => {
          const value = e.target.value
          writeComposeCaret(
            sessionId,
            e.target.selectionStart,
            e.target.selectionEnd,
            value.length,
          )
          if (historyIndex !== -1) {
            setHistoryIndex(-1)
            historyDraftRef.current = ''
          }
          const spaceCommit = composeSlashSpaceCommit(value)
          if (spaceCommit) {
            setSlashCommand(spaceCommit.command)
            setDraft(spaceCommit.remainder)
            closeSlashMenu()
            requestAnimationFrame(() => {
              const ta = textareaRef.current
              if (!ta) return
              ta.focus()
              ta.setSelectionRange(
                spaceCommit.remainder.length,
                spaceCommit.remainder.length,
              )
              writeComposeCaret(
                sessionId,
                spaceCommit.remainder.length,
                spaceCommit.remainder.length,
                spaceCommit.remainder.length,
              )
              autoGrow()
            })
            return
          }
          setDraft(value)
          const query = composeSlashTypeaheadQuery(value)
          if (query != null) {
            const matches = filterComposeSlashCommands(query)
            if (matches.length > 0) {
              if (!slashMenuOpen || slashMenuFromTypeahead) {
                placeSlashMenu()
                setSlashMenuFromTypeahead(true)
                if (!slashMenuOpen) setSlashHighlight(0)
                setSlashMenuOpen(true)
              }
            } else if (slashMenuFromTypeahead) {
              closeSlashMenu()
            }
          } else if (slashMenuFromTypeahead) {
            closeSlashMenu()
          }
        }}
        onSelect={persistCaret}
        onClick={persistCaret}
        onKeyUp={persistCaret}
        onBlur={persistCaret}
        onFocus={restoreCaret}
        onKeyDown={handleKeyDown}
        onDragOver={handleDragOver}
        onDrop={handleDrop}
        rows={1}
        spellCheck={false}
        placeholder={messagePlaceholder}
        title="Enter to send, Shift+Enter for newline. Drop files for paths."
        className="min-w-0 w-full flex-1 resize-none overflow-x-hidden bg-transparent text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none"
        style={{
          flex: '1 1 0%',
          minWidth: 0,
          width: '100%',
          fontFamily:
            "'MesloLGM Nerd Font', 'MesloLGM Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace",
          fontSize: editorFontSize,
          lineHeight: 1.4,
          boxSizing: 'border-box',
          border: 'none',
          borderRadius: 0,
          padding: '4px 6px',
          maxHeight: COMPOSE_TEXTAREA_MAX_HEIGHT,
          overflowY: 'auto',
          overflowX: 'hidden',
          overflowWrap: 'anywhere',
          wordBreak: 'break-word',
        }}
      />
      <button
        type="button"
        aria-label="Send message"
        title="Send"
        disabled={!canSend}
        onClick={() => void send()}
        className="inline-flex flex-shrink-0 items-center justify-center bg-[var(--color-accent)] text-white hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed transition-opacity"
        style={{
          borderRadius: 0,
          width: btnSize,
          height: btnSize,
          marginBottom: btnNudge,
        }}
      >
        <svg width={iconSize} height={iconSize} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M12 19V5" />
          <polyline points="5 12 12 5 19 12" />
        </svg>
      </button>
      </div>
      </div>
    </div>
  )
}

function ComposeImageThumb({
  path,
  onRemove,
}: {
  path: string
  onRemove: () => void
}): React.JSX.Element {
  const [url, setUrl] = useState<string | null>(null)
  const [open, setOpen] = useState(false)
  const urlRef = useRef<string | null>(null)

  useEffect(() => {
    let cancelled = false
    const ac = new AbortController()
    if (urlRef.current) {
      revokeObjectUrl(urlRef.current)
      urlRef.current = null
    }
    setUrl(null)
    void loadHostImageObjectUrl(path, { signal: ac.signal })
      .then((r) => {
        if (cancelled || ac.signal.aborted) {
          revokeObjectUrl(r.url)
          return
        }
        urlRef.current = r.url
        setUrl(r.url)
      })
      .catch(() => {
        if (!cancelled) setUrl(null)
      })
    return () => {
      cancelled = true
      ac.abort()
      if (urlRef.current) {
        revokeObjectUrl(urlRef.current)
        urlRef.current = null
      }
    }
  }, [path])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      setOpen(false)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [open])

  const name = path.split('/').pop()?.split('\\').pop() || path
  return (
    <>
    <div className="relative h-20 w-20 flex-shrink-0 overflow-hidden border border-[var(--color-border)] bg-[var(--color-bg)]" style={{ borderRadius: 0 }}>
      {url ? (
        <button
          type="button"
          className="h-full w-full cursor-zoom-in p-0 border-0 bg-transparent"
          aria-label={`View ${name}`}
          onClick={() => setOpen(true)}
        >
          <img src={url} alt={name} className="h-full w-full object-cover" />
        </button>
      ) : (
        <div className="h-full w-full bg-[var(--color-bg-hover)]" title={path} />
      )}
      <button
        type="button"
        aria-label={`Remove ${name}`}
        onClick={(e) => {
          e.stopPropagation()
          onRemove()
        }}
        className="absolute top-0 right-0 h-4 w-4 flex items-center justify-center bg-black/60 text-white text-[10px] leading-none hover:bg-black/80"
      >
        ×
      </button>
    </div>
    {open && url && createPortal(
      <div
        className="fixed inset-0 z-[99999] flex items-center justify-center bg-[var(--color-scrim)] p-6"
        role="dialog"
        aria-modal="true"
        aria-label={name}
        onClick={() => setOpen(false)}
      >
        <img
          src={url}
          alt={name}
          className="max-h-full max-w-full object-contain"
          style={{ borderRadius: 0 }}
          onClick={(e) => e.stopPropagation()}
        />
        <button
          type="button"
          aria-label="Close"
          onClick={() => setOpen(false)}
          className="absolute top-3 right-3 h-7 w-7 flex items-center justify-center bg-black/70 text-white text-lg leading-none hover:bg-black/90"
          style={{ borderRadius: 0 }}
        >
          ×
        </button>
      </div>,
      document.body,
    )}
    </>
  )
}
