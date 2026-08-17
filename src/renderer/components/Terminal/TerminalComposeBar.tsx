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
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import {
  buildComposeDropPayload,
  filesFromDataTransfer,
} from '@/lib/external-drop-router'
import { executeBrowserFileDrop, executeRemoteDrop } from '@/lib/handle-remote-drop'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { isEffectivelyHidden } from '@/lib/workspace-switch-focus'
import {
  type ComposeHistoryItem,
  type MsgResponse,
  applyComposeHistoryNav,
  composeHistoryKeyAction,
  composeInterruptSequence,
  composerPermitted,
  mapMsgResponseToStatus,
  shouldSendOnKey,
} from './terminalCompose'

const MAX_TEXTAREA_HEIGHT = 160 // px — auto-grow cap before internal scroll

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

  // Draft persistence (thin client): key the draft by this pane's PTY session
  // and back it with localStorage so switching workspaces/tabs restores each
  // composer's own text instead of clearing it, and a crash/restart never loses
  // it. Per-user/per-device by nature (localStorage is the desktop app's own
  // storage); the draft never touches the daemon.
  const draftKey = `k2:composer:draft:${sessionId}`
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
  const historyDraftRef = useRef('')
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const barRef = useRef<HTMLDivElement>(null)

  // Reload the saved draft when the pane's session changes — this component can
  // be reused with a new sessionId on a workspace/tab switch without remounting.
  useEffect(() => {
    try {
      setDraft(localStorage.getItem(draftKey) ?? '')
    } catch {
      setDraft('')
    }
    setHistoryIndex(-1)
    historyDraftRef.current = ''
  }, [draftKey])

  // Persist on every change (localStorage writes are cheap + synchronous — this
  // is the crash-durable store). An empty draft clears the key.
  useEffect(() => {
    try {
      if (draft) localStorage.setItem(draftKey, draft)
      else localStorage.removeItem(draftKey)
    } catch {
      /* storage disabled/full — draft just won't persist */
    }
  }, [draft, draftKey])

  // Auto-grow with content — same approach as ticket / project chat:
  // measure from `height: auto` so the resting size does not collapse when
  // the first character is typed (height: 0px was shrinking the field).
  const autoGrow = useCallback(() => {
    const el = textareaRef.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, MAX_TEXTAREA_HEIGHT)}px`
  }, [])

  useEffect(() => {
    autoGrow()
  }, [draft, autoGrow])

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
    })
    return () => cancelAnimationFrame(id)
  }, [sessionId])

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

  const send = useCallback(async () => {
    const text = draft.trim()
    if (!text || sending) return

    setSending(true)
    setDraft('') // optimistic clear (PRD 1b); restored below only on failure

    try {
      const resp = await daemonCliPost<MsgResponse>('terminal/send-message', {
        session_id: sessionId,
        text,
      })
      // Failed send → restore the text so it's not lost (the box reappearing
      // IS the feedback) — but never clobber a fresh draft already started.
      if (mapMsgResponseToStatus(resp).kind !== 'delivered') {
        setDraft((cur) => (cur.length === 0 ? text : cur))
      } else {
        setHistory((prev) => [text, ...prev].slice(0, 50))
        setHistoryIndex(-1)
        historyDraftRef.current = ''
      }
    } catch {
      setDraft((cur) => (cur.length === 0 ? text : cur))
    } finally {
      setSending(false)
    }
  }, [draft, sending, sessionId])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
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
    [send, draft, history, historyIndex, onInjectInput]
  )

  // 1c renderer-hide (after all hooks): not permitted → render nothing.
  if (!permitted) return null

  // One condensed row — just the textarea.
  // min-w-0 on the flex row + field so app zoom (Cmd+=) reflows width
  // instead of pinning content-min-width and growing a horizontal scrollbar.
  return (
    <div
      ref={barRef}
      className="flex min-w-0 w-full flex-shrink-0 items-start gap-1 border-t border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-2 pt-1.5 pb-2.5"
      data-compose-bar=""
      data-session-id={sessionId}
      data-workspace-path={workspacePath || undefined}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      <textarea
        ref={textareaRef}
        value={draft}
        onChange={(e) => {
          setDraft(e.target.value)
          if (historyIndex !== -1) {
            setHistoryIndex(-1)
            historyDraftRef.current = ''
          }
        }}
        onKeyDown={handleKeyDown}
        onDragOver={handleDragOver}
        onDrop={handleDrop}
        rows={1}
        spellCheck={false}
        placeholder="Message the agent — Enter to send, Shift+Enter for newline · drop files for paths"
        className="min-w-0 w-full flex-1 resize-none overflow-x-hidden bg-[var(--color-bg)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none"
        style={{
          fontFamily:
            "'MesloLGM Nerd Font', 'MesloLGM Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace",
          fontSize: editorFontSize,
          lineHeight: 1.4,
          border: '1px solid var(--color-border)',
          borderRadius: 0,
          padding: '4px 6px',
          maxHeight: MAX_TEXTAREA_HEIGHT,
          overflowY: 'auto',
          overflowX: 'hidden',
          // Break long tokens so zoom never forces a horizontal scrollbar.
          overflowWrap: 'anywhere',
          wordBreak: 'break-word',
        }}
      />
    </div>
  )
}
