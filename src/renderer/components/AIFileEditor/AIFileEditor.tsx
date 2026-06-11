import { useEffect, useMemo, useRef, useState, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { terminalKill } from '@/lib/terminal-daemon'
import { TerminalPane } from '@/terminal-v2/TerminalPane'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

// ── Session file helpers ────────────────────────────────────────────

const SESSION_FILE = '.last_editor_session'

async function readEditorSession(cwd: string): Promise<string | null> {
  try {
    const result = await daemonCliGet<{ content: string }>('fs/read-file', {
      path: `${cwd}/${SESSION_FILE}`,
    })
    const id = result.content.trim()
    return id || null
  } catch {
    return null
  }
}

async function saveEditorSession(cwd: string, command: string | undefined): Promise<void> {
  if (!command) return
  // Map command to provider name for session detection
  const provider = command === 'claude' ? 'claude' : command === 'cursor' ? 'cursor' : null
  if (!provider) return

  try {
    const r = await daemonCliGet<{ sessionId: string | null }>('chat/detect-active', {
      provider,
      project_path: cwd,
    })
    const sessionId = r.sessionId
    if (sessionId) {
      await daemonCliPost('fs/write-file', {
        path: `${cwd}/${SESSION_FILE}`,
        content: sessionId,
      })
    }
  } catch {
    // Non-fatal — session save is best-effort
  }
}

// ── Props ────────────────────────────────────────────────────────────

/** A single file tab when AIFileEditor is in multi-file mode. */
export interface EditorFile {
  /** Absolute path to this file */
  path: string
  /** Short tab label shown to the user (e.g., 'Persona', 'Wake-up') */
  label: string
}

interface AIFileEditorProps {
  /**
   * Absolute path to the file being edited. In multi-file mode this
   * is ignored in favor of `files[activeFileIndex].path`.
   */
  filePath: string
  /**
   * Optional list of files for multi-file editing. When provided,
   * renders a small tab strip; the parent's `onFileChange` is called
   * with the newly-read content of whichever file is currently active.
   * The AI terminal remains a single session so it can reason across
   * all files at once — the system prompt the parent supplies via
   * `args` should explain each file's purpose.
   */
  files?: EditorFile[]
  /** Directory to watch for changes (usually the parent of filePath) */
  watchDir: string
  /** Terminal working directory */
  cwd: string
  /** Command to auto-run in the terminal (e.g. 'claude') */
  command?: string
  /** Arguments for the command */
  args?: string[]
  /** Context shown above the terminal (what this editor is for) */
  instructions?: string
  /** Warning text for the banner (default provided) */
  warningText?: string
  /** Live preview content — any React node, managed by the parent */
  preview: React.ReactNode
  /**
   * Called when the watched file changes. In multi-file mode the
   * second argument is the path that changed so the parent can route
   * content to the right state slot.
   */
  onFileChange: (content: string, path?: string) => void
  /** Called when user clicks Back */
  onClose: () => void
  /** Top bar title */
  title?: string
  /** Optional manual refresh callback */
  onManualRefresh?: () => void
  /** Enable file rename detection — watcher scans watchDir for renamed files (default: false) */
  trackFileRename?: boolean
  /**
   * Called when the user clicks a file tab (multi-file mode only).
   * Parent can use this to swap its preview panel to match.
   */
  onActiveFileChange?: (path: string) => void
  /**
   * Render the file tab strip. Defaults to true when `files` is provided.
   * Set to false when the parent already has its own tab UI and is only
   * using `files` for multi-file watching.
   */
  showTabs?: boolean
  /**
   * Optional unsaved-changes signal from the parent's preview pane.
   * When true, the editor's header shows Save/Discard buttons and
   * the Back button surfaces a confirm dialog. When undefined (the
   * default), no Save/Discard UI appears — preserves the legacy
   * behavior for callers (CustomThemeCreator, AgentPersonaEditor)
   * that don't yet wire dirty state.
   */
  isDirty?: boolean
  /**
   * Called when the user clicks Save in the header. Implementations
   * typically forward to a `FileViewerHandle.save()` exposed by the
   * preview pane.
   */
  onSaveRequested?: () => Promise<void> | void
  /**
   * Called when the user clicks Discard in the header. Reverts the
   * preview's in-memory buffer to the on-disk content.
   */
  onDiscardRequested?: () => void
  /**
   * When true, skip the saved-session resume entirely — the terminal
   * always launches a FRESH agent session, ignoring `.last_editor_session`.
   * The K2 Canonical Agent modal sets this: its setup/unwind ceremony is a
   * one-shot that must not silently resume a stale prior conversation
   * (canonical-agents PRD §9.2).
   */
  disableSessionResume?: boolean
}

// ── Component ────────────────────────────────────────────────────────

export function AIFileEditor({
  filePath,
  files,
  watchDir,
  cwd,
  command,
  args,
  instructions,
  warningText = 'This is a real terminal with full system access. It can edit files beyond this one if given the wrong command.',
  preview,
  onFileChange,
  onClose,
  title = 'AI File Editor',
  onManualRefresh,
  trackFileRename = false,
  onActiveFileChange,
  showTabs,
  isDirty,
  onSaveRequested,
  onDiscardRequested,
  disableSessionResume = false,
}: AIFileEditorProps): React.JSX.Element {
  const terminalIdRef = useRef(`ai-editor-${crypto.randomUUID()}`)
  const [terminalReady, setTerminalReady] = useState(false)
  // In multi-file mode, `files[0]` is the initial active tab. In single-file
  // mode we fall back to the legacy `filePath` prop.
  const isMultiFile = !!files && files.length > 1
  const [activeFileIndex, setActiveFileIndex] = useState(0)
  const effectivePath = isMultiFile ? files![activeFileIndex]?.path ?? filePath : filePath
  const [activeFilePath, setActiveFilePath] = useState(effectivePath)

  // Sync with prop changes (e.g. tab switches from within multi-file mode,
  // or parent swapping the single-file filePath). The per-path Map in
  // the watcher handles cache invalidation for previously-unseen paths,
  // so no manual reset is needed here.
  useEffect(() => {
    setActiveFilePath(effectivePath)
  }, [effectivePath])
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // ── Session resume: detect previous editor session ────────────────
  const [resolvedArgs, setResolvedArgs] = useState<string[] | undefined>(undefined)
  const [argsReady, setArgsReady] = useState(false)
  const commandRef = useRef(command)
  commandRef.current = command

  useEffect(() => {
    let cancelled = false
    const resolve = async () => {
      if (!command || command !== 'claude' || disableSessionResume) {
        // Non-Claude commands, or callers that opt out of resume (the
        // canonical modal): use args as-is, never resume a prior session.
        setResolvedArgs(args)
        setArgsReady(true)
        return
      }

      // Check for a saved editor session to resume
      const savedSession = await readEditorSession(cwd)
      if (cancelled) return

      if (savedSession) {
        // Resume previous session: strip any existing --resume + sessionId pair, then add ours
        const raw = args ?? []
        const resumeIdx = raw.indexOf('--resume')
        const baseArgs = resumeIdx === -1
          ? raw
          : [...raw.slice(0, resumeIdx), ...raw.slice(resumeIdx + 2)]
        setResolvedArgs([...baseArgs, '--resume', savedSession])
      } else {
        setResolvedArgs(args)
      }
      setArgsReady(true)
    }
    resolve()
    return () => { cancelled = true }
  }, [command, args, cwd, disableSessionResume])

  // ── File watching + polling fallback ─────────────────────────────────
  // We watch every file the parent declared (via `files`) — not just the
  // currently-active tab — so the parent's preview can reflect external
  // edits to any tracked file (e.g. the AI terminal editing wakeup.md
  // while the user has the Persona tab in focus).
  const lastContentByPathRef = useRef<Map<string, string>>(new Map())
  const fileExtension = effectivePath.split('.').pop() || 'json'
  const watchedPaths = useMemo<string[]>(() => {
    if (files && files.length > 0) return files.map((f) => f.path)
    return [filePath]
  }, [files, filePath])
  // Stable dep for the watcher effect — re-run only when the set of
  // watched paths actually changes, not on every render.
  const watchedKey = watchedPaths.join('|')

  useEffect(() => {
    let unlisten: (() => void) | null = null

    const readOne = async (path: string) => {
      try {
        const result = await daemonCliGet<{ content: string; path: string; name: string }>('fs/read-file', { path })
        const prev = lastContentByPathRef.current.get(path)
        if (result.content !== prev) {
          lastContentByPathRef.current.set(path, result.content)
          onFileChange(result.content, path)
        }
      } catch {
        // File may be mid-write or temporarily missing — ignore
      }
    }

    const findAndRead = async () => {
      if (trackFileRename) {
        // Scan directory for renamed files — used by theme editor etc.
        try {
          const entries = await invoke<{ name: string; path: string; isDirectory: boolean; modifiedAt: number }[]>(
            'fs_read_dir', { path: watchDir }
          )
          const matching = entries
            .filter((e) => !e.isDirectory && e.name.endsWith(`.${fileExtension}`) && !e.name.startsWith('.'))
            .sort((a, b) => b.modifiedAt - a.modifiedAt)

          const target = matching[0]
          if (!target) return

          const originalStillExists = entries.some((e) => e.path === filePath)
          if (!originalStillExists && target.path !== filePath) {
            setActiveFilePath(target.path)
          }
          await readOne(target.path)
        } catch {
          // Directory may not exist yet — ignore
        }
        return
      }
      // Normal mode: re-read every tracked file, fire onFileChange only
      // for those whose content actually changed since last read.
      await Promise.all(watchedPaths.map(readOne))
    }

    invoke('fs_watch_dir', { path: watchDir }).catch((err) => {
      console.warn('[ai-editor] Failed to start watcher:', err)
    })

    // Accept either single-event (pre-0.32.13) or batched-array payloads.
    // This editor just debounces a reload on any change, so we don't need
    // to inspect the batch contents — just knowing "something changed"
    // is enough.
    listen<Array<{ path: string; kind: string }> | { path: string; kind: string }>('fs://change', () => {
      if (debounceRef.current) clearTimeout(debounceRef.current)
      debounceRef.current = setTimeout(findAndRead, 300)
    }).then((fn) => { unlisten = fn })

    const pollInterval = setInterval(findAndRead, 2000)

    return () => {
      unlisten?.()
      clearInterval(pollInterval)
      if (debounceRef.current) clearTimeout(debounceRef.current)
      invoke('fs_unwatch_dir', { path: watchDir }).catch((e) => console.warn('[ai-editor]', e))
    }
  }, [watchDir, watchedKey, fileExtension, trackFileRename, onFileChange, filePath, watchedPaths])

  // ── Terminal cleanup on unmount ────────────────────────────────────
  const cwdRef = useRef(cwd)
  cwdRef.current = cwd

  useEffect(() => {
    const id = terminalIdRef.current
    const t = setTimeout(() => setTerminalReady(true), 100)
    return () => {
      clearTimeout(t)
      // Save session before killing so it can be resumed next time
      saveEditorSession(cwdRef.current, commandRef.current).finally(() => {
        terminalKill(id).catch(() => {})
      })
    }
  }, [])

  // Explicit close: save session, kill terminal, then notify parent.
  // If the parent has signaled `isDirty`, prompt before discarding —
  // gives the user a chance to keep edits the AI just made or undo
  // them, rather than silently losing both on a click of Back.
  const handleClose = useCallback(async () => {
    if (isDirty) {
      const choice = window.confirm(
        'You have unsaved changes.\n\nClick OK to save and exit.\nClick Cancel to stay (use Discard then Back if you want to drop the changes).',
      )
      if (!choice) return
      if (onSaveRequested) {
        try {
          await onSaveRequested()
        } catch (e) {
          console.error('[ai-file-editor] save before close failed:', e)
        }
      }
    }
    await saveEditorSession(cwd, commandRef.current)
    terminalKill(terminalIdRef.current).catch(() => {})
    onClose()
  }, [cwd, onClose, isDirty, onSaveRequested])

  const fileName = filePath.split('/').pop() || 'file'

  return (
    <div className="flex flex-col h-full">
      {/* ── Top bar ── */}
      <div className="flex items-center gap-3 px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
        <button
          onClick={handleClose}
          className="text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer no-drag"
        >
          &larr; Back
        </button>
        <span className="text-xs font-medium text-[var(--color-text-primary)] flex items-center gap-1.5">
          {title}
          {/* Dirty indicator (vim-style dot) — only when parent wires
              isDirty. Hidden otherwise so existing callers without
              dirty-state plumbing render unchanged. */}
          {isDirty && (
            <span
              className="inline-block w-1.5 h-1.5 rounded-full bg-amber-400"
              title="Unsaved changes"
            />
          )}
        </span>
        <span className="text-[10px] font-mono text-[var(--color-text-muted)] ml-auto truncate max-w-[40%]">
          {activeFilePath}
        </span>
        {/* Save / Discard appear only when the parent supplies the
            handlers — keeps existing AIFileEditor consumers
            (CustomThemeCreator, AgentPersonaEditor) untouched. */}
        {onSaveRequested && (
          <button
            onClick={() => { void onSaveRequested() }}
            disabled={!isDirty}
            title={isDirty ? 'Save changes to disk' : 'No unsaved changes'}
            className="px-2 py-0.5 text-[10px] font-medium text-white bg-[var(--color-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-30 disabled:cursor-not-allowed flex-shrink-0"
          >
            Save
          </button>
        )}
        {onDiscardRequested && (
          <button
            onClick={() => { onDiscardRequested() }}
            disabled={!isDirty}
            title={isDirty ? 'Revert to the last saved version (drops user + AI edits)' : 'No unsaved changes'}
            className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-secondary)] transition-colors cursor-pointer no-drag disabled:opacity-30 disabled:cursor-not-allowed flex-shrink-0"
          >
            Discard
          </button>
        )}
        {onManualRefresh && (
          <button
            onClick={onManualRefresh}
            className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-secondary)] transition-colors cursor-pointer no-drag flex-shrink-0"
          >
            Refresh Preview
          </button>
        )}
      </div>

      {/* ── Warning banner ── */}
      <div className="flex items-center gap-2 px-4 py-2 bg-amber-500/8 border-b border-amber-500/20 flex-shrink-0">
        <span className="text-amber-400 text-sm flex-shrink-0">&#9888;</span>
        <span className="text-[11px] text-amber-300/80 leading-relaxed">{warningText}</span>
      </div>

      {/* ── File tabs (multi-file mode only; parent can hide via showTabs=false) ── */}
      {isMultiFile && files && (showTabs ?? true) && (
        <div className="flex items-center gap-1 px-4 py-1.5 border-b border-[var(--color-border)] flex-shrink-0">
          {files.map((f, i) => (
            <button
              key={f.path}
              onClick={() => {
                setActiveFileIndex(i)
                onActiveFileChange?.(f.path)
              }}
              className={`px-2.5 py-1 text-[11px] font-medium transition-colors no-drag cursor-pointer ${
                i === activeFileIndex
                  ? 'text-[var(--color-text-primary)] border-b-2 border-[var(--color-accent)]'
                  : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
              }`}
              title={f.path}
            >
              {f.label}
            </button>
          ))}
        </div>
      )}

      {/* ── Instructions (optional) ── */}
      {instructions && (
        <div className="px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
          <p className="text-[11px] text-[var(--color-text-muted)] leading-relaxed">{instructions}</p>
        </div>
      )}

      {/* ── Split view: terminal + preview ── */}
      <div className="flex flex-1 min-h-0">
        {/* Terminal (left) — wait for session resolution before mounting */}
        <div className="flex-1 min-w-0 border-r border-[var(--color-border)]">
          {argsReady ? (
            // Hardcoded to v2 — system-driven mount inside the file
            // editor UI (not a workspace tab), so bypasses the user's
            // Settings → Renderer choice. See A8 plan.
            <TerminalPane
              terminalId={terminalIdRef.current}
              cwd={cwd}
              command={command}
              args={resolvedArgs}
            />
          ) : (
            <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
              Checking for previous session...
            </div>
          )}
        </div>

        {/* Preview (right) */}
        <div className="flex-1 min-w-0 overflow-hidden">
          {preview}
        </div>
      </div>
    </div>
  )
}
