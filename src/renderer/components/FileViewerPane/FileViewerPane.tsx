import React, { Suspense, lazy, useState, useEffect, useCallback, useRef, useLayoutEffect, useImperativeHandle, useMemo } from 'react'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'
import { invoke, convertFileSrc } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
// 0.39.0 bundle-perf: PDFViewer pulls in pdfjs-dist (~600KB gzip),
// DocxViewer pulls in mammoth (~200KB), CodeEditor pulls in
// @codemirror/* (~100KB). Lazy-load all three so they only enter the
// bundle when the user actually opens a matching file. The Suspense
// fallback shows briefly during the dynamic import — see the JSX
// usages below.
const PDFViewer = lazy(() => import('./PDFViewer').then((m) => ({ default: m.PDFViewer })))
const DocxViewer = lazy(() => import('./DocxViewer').then((m) => ({ default: m.DocxViewer })))
const CodeEditor = lazy(() => import('./CodeEditor').then((m) => ({ default: m.CodeEditor })))
// `getLanguageName` is a synchronous helper used in the status bar —
// importing it eagerly is fine (it's a small string table, not the
// CodeMirror bundle). Bundlers tree-shake the rest of the module.
import { getLanguageName } from './CodeEditor'
import { HighlightedCodeBlock } from './CodeHighlighter'
import { DiffViewer } from '@/components/DiffViewer/DiffViewer'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { FILE_POLL_INTERVAL } from '@shared/constants'

// ── Types ────────────────────────────────────────────────────────────────

/**
 * Imperative handle exposed by FileViewerPane via the optional
 * `commandRef` prop. Lets a parent (e.g. AIFileEditor) wire its own
 * Save/Discard buttons to the editor's dirty buffer without having to
 * mirror all of the pane's internal state.
 */
export interface FileViewerHandle {
  save: () => Promise<void>
  discard: () => void
}

interface FileViewerPaneProps {
  filePath: string
  mode?: 'edit' | 'diff'
  paneId: string
  paneGroupId?: string
  tabId: string
  initialScrollTop?: number
  initialCursorPos?: number
  onClose?: () => void
  /** Imperative handle for parent-controlled Save/Discard. The
   *  FileViewerPane writes into this ref on mount; parents can call
   *  `commandRef.current?.save()` / `.discard()` from their own UI. */
  commandRef?: React.RefObject<FileViewerHandle | null>
  /** Notifies the parent on every dirty-state transition. Lets the
   *  parent enable/disable a Save button or show an unsaved-changes
   *  indicator without coupling to the internal state. */
  onDirtyChange?: (isDirty: boolean) => void
}

// Pure classification helpers live in fileCategory.ts (no React/Tauri
// deps) so they're unit-testable in isolation. Re-exported here so
// existing import sites (and the #587 tests) keep working.
import {
  getFileCategory,
  getDefaultViewMode,
  type FileCategory,
  type ViewMode,
} from './fileCategory'
export { getFileCategory, getDefaultViewMode } from './fileCategory'
export type { FileCategory, ViewMode } from './fileCategory'

/** True if the current document selection overlaps `container` and
 *  has non-empty text. Used to suppress the markdown-preview
 *  auto-refresh while the user is dragging or copying — ReactMarkdown
 *  re-renders rebuild the DOM and the selection collapses, which
 *  silently breaks Cmd+C from the preview. */
function hasSelectionWithin(container: HTMLElement | null): boolean {
  if (!container) return false
  const sel = window.getSelection()
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return false
  if (sel.toString().length === 0) return false
  for (let i = 0; i < sel.rangeCount; i++) {
    const range = sel.getRangeAt(i)
    if (
      container.contains(range.startContainer) ||
      container.contains(range.endContainer)
    ) {
      return true
    }
  }
  return false
}

/**
 * Module-level ReactMarkdown components map. Hoisted out of the
 * render function so its reference is stable across renders — if
 * we rebuilt this object inside the component body, ReactMarkdown
 * would get a new prop ref every render and rebuild its element
 * tree, which destroys any active DOM text selection (the user
 * loses their highlight every time the parent re-renders).
 */
const MARKDOWN_COMPONENTS = {
  code({ className, children, ...props }: React.HTMLAttributes<HTMLElement> & { children?: React.ReactNode }) {
    const match = /language-(\w+)/.exec(className || '')
    const codeStr = String(children).replace(/\n$/, '')
    if (match) {
      return <HighlightedCodeBlock code={codeStr} language={match[1]} />
    }
    return <code className={className} {...props}>{children}</code>
  },
}
const MARKDOWN_REMARK_PLUGINS = [remarkGfm]

/**
 * Memoized markdown body. Only re-renders when `content` actually
 * changes — without this, every parent FileViewerPane re-render
 * (for any unrelated reason: dirty-state ticks, settings polls,
 * tab activity, etc.) rebuilt ReactMarkdown's element tree, and
 * React's reconciliation rebuilt enough of the DOM that the
 * browser's active text selection collapsed. Wrapping in
 * React.memo with default shallow prop comparison stops the
 * cascade — content only changes when the file changes on disk
 * (or the user types in edit mode).
 */
const MemoizedMarkdown = React.memo(function MemoizedMarkdown({
  content,
}: {
  content: string
}): React.JSX.Element {
  return (
    <Markdown
      remarkPlugins={MARKDOWN_REMARK_PLUGINS}
      components={MARKDOWN_COMPONENTS}
    >
      {content}
    </Markdown>
  )
})

function getFileName(filePath: string): string {
  return filePath.split('/').pop() || filePath
}

function getShortPath(filePath: string): string {
  if (filePath.length > 60) return '...' + filePath.slice(-57)
  return filePath
}

// ── Error Boundary ───────────────────────────────────────────────────────

class EditorErrorBoundary extends React.Component<
  { filePath: string; children: React.ReactNode },
  { hasError: boolean; error: string | null }
> {
  constructor(props: { filePath: string; children: React.ReactNode }) {
    super(props)
    this.state = { hasError: false, error: null }
  }
  static getDerivedStateFromError(error: Error) {
    return { hasError: true, error: error.message }
  }
  render() {
    if (this.state.hasError) {
      return (
        <div className="h-full flex items-center justify-center text-[var(--color-text-muted)]">
          <div className="text-center space-y-2">
            <p className="text-sm">Failed to load editor</p>
            <p className="text-xs font-mono opacity-60">{this.state.error}</p>
            <p className="text-xs opacity-40">{this.props.filePath}</p>
          </div>
        </div>
      )
    }
    return this.props.children
  }
}

// ── Component ────────────────────────────────────────────────────────────

export function FileViewerPane(props: FileViewerPaneProps): React.JSX.Element {
  const { filePath, mode, paneId, paneGroupId, tabId, initialScrollTop, initialCursorPos, onClose, commandRef, onDirtyChange } = props
  // Diff mode — render DiffViewer instead of editor
  if (mode === 'diff') {
    return <DiffViewer filePath={filePath} className="h-full" />
  }

  return (
    <EditorErrorBoundary filePath={filePath}>
      <FileViewerPaneInner
        filePath={filePath}
        paneId={paneId}
        paneGroupId={paneGroupId}
        tabId={tabId}
        initialScrollTop={initialScrollTop}
        initialCursorPos={initialCursorPos}
        onClose={onClose}
        commandRef={commandRef}
        onDirtyChange={onDirtyChange}
      />
    </EditorErrorBoundary>
  )
}

function FileViewerPaneInner({ filePath, paneId, paneGroupId, tabId, initialScrollTop, initialCursorPos, onClose, commandRef, onDirtyChange }: Omit<FileViewerPaneProps, 'mode'>): React.JSX.Element {
  const [content, setContent] = useState<string>('')
  const [editedContent, setEditedContent] = useState<string | null>(null) // null = not edited
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [cursorInfo, setCursorInfo] = useState({ line: 1, col: 1, selections: 1 })

  const category = getFileCategory(filePath)
  const [viewMode, setViewMode] = useState<ViewMode>(getDefaultViewMode(category))
  const isDirty = editedContent !== null && editedContent !== content
  const setTabDirty = useTabsStore((s) => s.setTabDirty)
  const setFileViewerState = useTabsStore((s) => s.setFileViewerState)

  const pinned = useTabsStore((s) => {
    const tab = s.tabs.find((t) => t.id === tabId)
    if (!tab) return false
    // Search all paneGroups for an item matching paneId
    for (const [, pg] of tab.paneGroups) {
      for (const item of pg.items) {
        if (item.id === paneId && item.type === 'file-viewer') {
          return item.pinned ?? false
        }
      }
    }
    return false
  })
  const pinPane = useTabsStore((s) => s.pinPane)
  const unpinPane = useTabsStore((s) => s.unpinPane)

  // #587 — HTML files get a *top-level tab* pin (next to the Inbox)
  // instead of the within-pane preview pin. `filePin` reflects whether
  // this file is currently in the workspace's pinned-HTML tab list;
  // the toolbar pin button toggles it for html files.
  const filePin = useTabsStore((s) =>
    s.tabs.some((t) => {
      if (!t.isPinnedFile) return false
      const item = Array.from(t.paneGroups.values())[0]?.items[0]
      if (item?.type !== 'file-viewer') return false
      return (item.data as { filePath?: string }).filePath === filePath
    }),
  )
  const pinFileAsTab = useTabsStore((s) => s.pinFileAsTab)
  const unpinFileTab = useTabsStore((s) => s.unpinFileTab)

  const [searchQuery, setSearchQuery] = useState('')
  const [searchVisible, setSearchVisible] = useState(false)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)
  const editorContainerRef = useRef<HTMLDivElement>(null)
  // Root container of the entire pane — covers the rendered preview,
  // toolbar, AND the filepath bar at the bottom. Used by the
  // auto-refresh selection guard so a user highlighting the filename
  // (or any other text in the pane chrome) doesn't have their
  // selection wiped by the next poll's re-render.
  const rootRef = useRef<HTMLDivElement>(null)


  const fileName = getFileName(filePath)
  const shortPath = getShortPath(filePath)

  // Reset view mode when file changes
  useEffect(() => {
    const newCategory = getFileCategory(filePath)
    setViewMode(getDefaultViewMode(newCategory))
  }, [filePath])

  const loadFile = useCallback(async () => {
    // Images, PDFs, and DOCX files don't need text content
    if (getFileCategory(filePath) === 'image' || getFileCategory(filePath) === 'pdf' || getFileCategory(filePath) === 'docx') {
      setLoading(false)
      setError(null)
      return
    }

    setLoading(true)
    setError(null)
    try {
      const result = await daemonCliGet<{ content: string }>('fs/read-file', { path: filePath })
      setContent(result.content)
    } catch {
      // File doesn't exist yet (e.g., untitled document) — start with empty content
      setContent('')
    } finally {
      setLoading(false)
    }
  }, [filePath])

  useEffect(() => {
    loadFile()
  }, [loadFile])

  // Sync dirty state to tab
  useEffect(() => {
    setTabDirty(tabId, isDirty)
  }, [isDirty, tabId, setTabDirty])

  // Scroll persistence for markdown/preview views.
  //
  // In-session tab switches don't remount this component (see
  // PaneGroupView — all pane items stay mounted, only visibility
  // toggles), so the browser preserves scrollTop on the DOM natively.
  // No restore dance needed.
  //
  // We still capture scroll position in a ref on every scroll and flush
  // to the store on unmount / file change, so it can be serialized for
  // session restore across app relaunches. `initialScrollTop` is used
  // as a first-mount hint when hydrating a saved workspace session.
  useLayoutEffect(() => {
    if (loading) return
    const el = contentRef.current
    if (!el) return

    // Seed latestScrollTop with the saved position so the unmount flush
    // preserves it even if the user never scrolled this session.
    let latestScrollTop = typeof initialScrollTop === 'number' ? initialScrollTop : el.scrollTop
    const handleScroll = (): void => {
      if (contentRef.current) latestScrollTop = contentRef.current.scrollTop
    }
    el.addEventListener('scroll', handleScroll, { passive: true })

    // One-shot restore for session-hydrated workspaces: if the store
    // has a saved scroll but the DOM is at 0 (fresh mount after relaunch),
    // apply it. Deferred to next frame for async-rendered content. If
    // the DOM already has a scroll (tab was mounted in this session and
    // user has scrolled), this is a no-op.
    const target = typeof initialScrollTop === 'number' && initialScrollTop > 0 ? initialScrollTop : 0
    if (target > 0 && el.scrollTop === 0) {
      requestAnimationFrame(() => {
        if (contentRef.current && contentRef.current.scrollTop === 0) {
          contentRef.current.scrollTop = target
        }
      })
    }

    return () => {
      el.removeEventListener('scroll', handleScroll)
      if (paneGroupId && latestScrollTop > 0) {
        setFileViewerState(tabId, paneGroupId, paneId, { scrollTop: latestScrollTop })
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loading, viewMode, filePath])

  // Auto-refresh: poll for file changes every 2 seconds (only when not editing)
  useEffect(() => {
    if (getFileCategory(filePath) === 'image' || getFileCategory(filePath) === 'pdf' || getFileCategory(filePath) === 'docx') return
    if (isDirty) return // Don't overwrite user edits

    const interval = setInterval(async () => {
      // Skip the refresh while the user has an active text selection
      // anywhere inside this pane — the rendered markdown body, the
      // filename bar at the bottom, the toolbar, etc. React re-renders
      // collapse selection ranges, so polling would silently break
      // Cmd+C copy. Scoped to `rootRef` so a selection in another tab
      // or pane doesn't block this pane's refresh. The next poll cycle
      // picks up any disk changes once the user releases the selection.
      if (hasSelectionWithin(rootRef.current)) {
        return
      }
      try {
        const result = await daemonCliGet<{ content: string }>('fs/read-file', { path: filePath })
        // Functional update avoids stale closure over content
        setContent(prev => result.content !== prev ? result.content : prev)
      } catch {
        // Ignore polling errors
      }
    }, FILE_POLL_INTERVAL)

    return () => clearInterval(interval)
  }, [filePath, isDirty])

  // Save file (Cmd+S) — called directly by CodeEditor with current content
  const saveFile = useCallback(async (contentToSave?: string) => {
    const toSave = contentToSave ?? editedContent
    if (toSave === null || toSave === undefined) return
    if (toSave === content) return // Nothing changed
    setSaving(true)
    try {
      await daemonCliPost('fs/write-file', { path: filePath, content: toSave })
      setContent(toSave)
      setEditedContent(null)
    } catch (err) {
      console.error('[file-viewer] Save failed:', err)
    } finally {
      setSaving(false)
    }
  }, [filePath, editedContent, content])

  // Discard unsaved edits — drops the in-memory edited buffer so the
  // pane re-renders against the on-disk content. Used by the parent's
  // optional Discard button (e.g. AIFileEditor's heartbeat editor).
  const discardEdits = useCallback(() => {
    setEditedContent(null)
  }, [])

  // Imperative handle for parent-controlled Save/Discard. The handle
  // re-binds whenever saveFile/discardEdits change so the parent's
  // ref always points at fresh closures (i.e. the latest editedContent
  // is captured when save() is called).
  useImperativeHandle(commandRef, () => ({
    save: () => saveFile(),
    discard: () => discardEdits(),
  }), [saveFile, discardEdits])

  // Notify parent on every dirty-state transition.
  useEffect(() => {
    onDirtyChange?.(isDirty)
  }, [isDirty, onDirtyChange])

  // Cmd+F search (only in rendered/markdown mode — CodeMirror handles its own search in raw mode)
  // Cmd+S save shortcut
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent): void => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'f' && viewMode === 'rendered') {
        e.preventDefault()
        e.stopPropagation()
        setSearchVisible(true)
        requestAnimationFrame(() => {
          searchInputRef.current?.focus()
          searchInputRef.current?.select()
        })
      }
      if ((e.metaKey || e.ctrlKey) && e.key === 's') {
        e.preventDefault()
        e.stopPropagation()
        saveFile(editedContent ?? undefined)
      }
      if (e.key === 'Escape' && searchVisible) {
        setSearchVisible(false)
        setSearchQuery('')
      }
    }

    const container = contentRef.current?.parentElement
    if (container) {
      container.addEventListener('keydown', handleKeyDown)
      return () => container.removeEventListener('keydown', handleKeyDown)
    }
  }, [searchVisible, saveFile, editedContent, viewMode])

  // Highlight search matches
  useEffect(() => {
    if (!contentRef.current || !searchQuery) return

    const existing = contentRef.current.querySelectorAll('mark[data-fv-search]')
    existing.forEach((el) => {
      const parent = el.parentNode
      if (parent) {
        parent.replaceChild(document.createTextNode(el.textContent || ''), el)
        parent.normalize()
      }
    })

    if (!searchQuery.trim()) return

    const walker = document.createTreeWalker(contentRef.current, NodeFilter.SHOW_TEXT)
    const matches: { node: Text; index: number }[] = []
    const query = searchQuery.toLowerCase()

    let node: Text | null
    while ((node = walker.nextNode() as Text | null)) {
      const text = node.textContent?.toLowerCase() || ''
      let idx = text.indexOf(query)
      while (idx !== -1) {
        matches.push({ node, index: idx })
        idx = text.indexOf(query, idx + 1)
      }
    }

    for (let i = matches.length - 1; i >= 0; i--) {
      const { node: textNode, index } = matches[i]
      const range = document.createRange()
      range.setStart(textNode, index)
      range.setEnd(textNode, index + searchQuery.length)

      const mark = document.createElement('mark')
      mark.setAttribute('data-fv-search', '')
      mark.style.background = '#b5890066'
      mark.style.color = 'inherit'
      mark.style.borderRadius = '0'
      range.surroundContents(mark)
    }

    const firstMark = contentRef.current.querySelector('mark[data-fv-search]')
    if (firstMark) {
      firstMark.scrollIntoView({ block: 'center', behavior: 'smooth' })
    }
  }, [searchQuery, content, viewMode])

  const handleTogglePin = useCallback(() => {
    // #587 — HTML files pin to a TOP-LEVEL tab (next to the Inbox), not
    // the within-pane preview pin. Toggling here adds/removes the file
    // from the workspace's pinned-HTML tab list.
    if (category === 'html') {
      if (filePin) {
        unpinFileTab(filePath)
      } else {
        pinFileAsTab(filePath)
      }
      return
    }
    if (pinned) {
      unpinPane(tabId, paneId)
    } else {
      pinPane(tabId, paneId)
    }
  }, [category, filePin, filePath, pinFileAsTab, unpinFileTab, pinned, tabId, paneId, pinPane, unpinPane])

  // Show toggle only for markdown, html, and image files (not PDF).
  // HTML renders the same Preview / Edit toggle in the top bar as
  // markdown — Preview shows the sandboxed iframe, Edit shows the raw
  // source in CodeEditor.
  const showViewToggle = category === 'markdown' || category === 'html' || category === 'image'

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[#0a0a0a] text-[var(--color-text-muted)] text-sm">
        Loading...
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex h-full w-full flex-col bg-[#0a0a0a]">
        {/* Toolbar even in error state */}
        <div className="flex items-center gap-2 border-b border-[var(--color-border)] bg-[#111111] px-3 py-1.5 flex-shrink-0">
          <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
            {fileName}
          </span>
          <div className="flex-1" />
          {onClose && (
            <button
              className="p-1 text-[var(--color-text-muted)] hover:text-red-400 transition-colors"
              onClick={onClose}
              title="Close"
            >
              <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
                <line x1="18" y1="6" x2="6" y2="18" />
                <line x1="6" y1="6" x2="18" y2="18" />
              </svg>
            </button>
          )}
        </div>
        <div className="flex flex-1 flex-col items-center justify-center gap-3">
          <span className="text-red-400 text-sm">{error}</span>
          <button
            className="text-xs text-[var(--color-accent)] hover:underline"
            onClick={loadFile}
          >
            Retry
          </button>
        </div>
      </div>
    )
  }

  return (
    <div ref={rootRef} className="flex h-full w-full flex-col bg-[#0a0a0a] no-drag" tabIndex={-1}>
      {/* Toolbar */}
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] bg-[#111111] px-3 py-1.5 flex-shrink-0">
        {/* File info */}
        <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
          {fileName}
        </span>

        {/* Dirty / saving indicator */}
        {isDirty && (
          <span className="text-[10px] text-[var(--color-accent)] flex-shrink-0">
            {saving ? 'Saving...' : 'Modified'}
          </span>
        )}

        <div className="flex-1" />

        {/* Search toggle — in edit mode, opens CodeMirror's built-in search panel */}
        <button
          className={`p-1 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors ${searchVisible ? 'text-[var(--color-accent)]' : ''}`}
          onClick={() => {
            if (viewMode === 'edit') {
              // Dispatch Cmd+F into the CodeMirror editor to open its search panel
              const cmEl = editorContainerRef.current?.querySelector('.cm-editor')
              if (cmEl) {
                cmEl.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', metaKey: true, bubbles: true }))
              }
            } else {
              setSearchVisible(!searchVisible)
              if (!searchVisible) {
                requestAnimationFrame(() => searchInputRef.current?.focus())
              } else {
                setSearchQuery('')
              }
            }
          }}
          title="Search (Cmd+F)"
        >
          <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
        </button>

        {/* View mode toggle (markdown and images only) */}
        {showViewToggle && (
          <div className="flex border border-[var(--color-border)]">
            <button
              className={`px-2 py-0.5 text-[10px] font-medium transition-colors ${
                viewMode === 'rendered'
                  ? 'bg-[var(--color-accent)] text-white'
                  : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
              }`}
              onClick={() => setViewMode('rendered')}
            >
              Preview
            </button>
            <button
              className={`px-2 py-0.5 text-[10px] font-medium transition-colors ${
                viewMode === 'raw'
                  ? 'bg-[var(--color-accent)] text-white'
                  : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
              }`}
              onClick={() => setViewMode('raw')}
            >
              Edit
            </button>
          </div>
        )}

        {/* Word wrap toggle (only for text/code files) */}
        {category === 'text' || ((category === 'markdown' || category === 'html') && viewMode === 'raw') ? (
          <button
            className={`p-1 transition-colors ${
              useSettingsStore.getState().editor.wordWrap
                ? 'text-[var(--color-accent)]'
                : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
            }`}
            onClick={() => {
              const current = useSettingsStore.getState().editor.wordWrap
              useSettingsStore.getState().updateEditorSettings({ wordWrap: !current })
            }}
            title="Toggle word wrap"
          >
            <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
              <path d="M3 6h18" />
              <path d="M3 12h15a3 3 0 1 1 0 6h-4" />
              <path d="m16 16-2 2 2 2" />
              <path d="M3 18h7" />
            </svg>
          </button>
        ) : null}

        {/* Refresh */}
        <button
          className="p-1 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
          onClick={loadFile}
          title="Refresh"
        >
          <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
            <path d="M21.5 2v6h-6" />
            <path d="M2.5 22v-6h6" />
            <path d="M22 11.5A10 10 0 003.2 7.2" />
            <path d="M2 12.5a10 10 0 0018.8 4.3" />
          </svg>
        </button>

        {/* Pin/Unpin — for HTML this is the top-level tab pin (next to
            the Inbox); for other files it's the within-pane preview pin. */}
        <button
          className={`p-1 transition-colors ${
            (category === 'html' ? filePin : pinned)
              ? 'text-[var(--color-accent)]'
              : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
          }`}
          onClick={handleTogglePin}
          title={
            category === 'html'
              ? (filePin ? 'Unpin tab' : 'Pin as tab (next to Inbox)')
              : (pinned ? 'Unpin (preview mode)' : 'Pin (keep open)')
          }
        >
          <svg className="w-3.5 h-3.5" viewBox="0 0 16 16" fill="currentColor">
            <path d="M9.828.722a.5.5 0 0 1 .354.146l4.95 4.95a.5.5 0 0 1-.707.707l-.71-.71-3.18 3.18a3.5 3.5 0 0 1-.4.3L11 11.106V14.5a.5.5 0 0 1-.854.354L7.5 12.207 4.854 14.854a.5.5 0 0 1-.708-.708L6.793 11.5 4.146 8.854A.5.5 0 0 1 4.5 8h3.394a3.5 3.5 0 0 0 .3-.4l3.18-3.18-.71-.71a.5.5 0 0 1 .354-.854z" />
          </svg>
        </button>

        {/* Close */}
        {onClose && (
          <button
            className="p-1 text-[var(--color-text-muted)] hover:text-red-400 transition-colors"
            onClick={onClose}
            title="Close"
          >
            <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          </button>
        )}
      </div>

      {/* Search bar (rendered/preview mode only — edit mode uses CodeMirror's built-in search) */}
      {searchVisible && viewMode !== 'edit' && (
        <div className="flex items-center gap-2 border-b border-[var(--color-border)] bg-[#111111] px-3 py-1.5 flex-shrink-0">
          <svg className="w-3 h-3 text-[var(--color-text-muted)] flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
          <input
            ref={searchInputRef}
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Search in document..."
            className="flex-1 bg-transparent text-xs text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none"
          />
          {searchQuery && (
            <button
              className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
              onClick={() => setSearchQuery('')}
            >
              <svg className="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2}>
                <line x1="18" y1="6" x2="6" y2="18" />
                <line x1="6" y1="6" x2="18" y2="18" />
              </svg>
            </button>
          )}
        </div>
      )}

      {/* Content */}
      {category === 'pdf' ? (
        <div className="flex-1 overflow-hidden">
          <Suspense fallback={<div className="p-4 text-xs text-[var(--color-text-muted)]">Loading PDF…</div>}>
            <PDFViewer filePath={filePath} />
          </Suspense>
        </div>
      ) : category === 'docx' ? (
        <div className="flex-1 overflow-hidden">
          <Suspense fallback={<div className="p-4 text-xs text-[var(--color-text-muted)]">Loading document…</div>}>
            <DocxViewer filePath={filePath} />
          </Suspense>
        </div>
      ) : category === 'image' && viewMode === 'rendered' ? (
        <div className="flex-1 overflow-y-auto overflow-x-hidden" ref={contentRef}>
          <div className="flex items-center justify-center p-4 min-h-full bg-[#0a0a0a]">
            <img
              src={convertFileSrc(filePath)}
              alt={fileName}
              style={{ maxWidth: '100%', maxHeight: '100%', objectFit: 'contain' }}
              onError={(e) => {
                (e.target as HTMLImageElement).style.display = 'none'
                setError('Failed to load image')
              }}
            />
          </div>
        </div>
      ) : category === 'image' && viewMode === 'raw' ? (
        <div className="flex-1 overflow-y-auto overflow-x-hidden" ref={contentRef}>
          <div className="p-4 text-xs text-[var(--color-text-muted)]">
            <p>Binary image file. Switch to Preview mode to view.</p>
          </div>
        </div>
      ) : category === 'markdown' && viewMode === 'rendered' ? (
        <div className="flex-1 overflow-y-auto overflow-x-hidden" ref={contentRef}>
          <div className="markdown-content p-4" style={{ userSelect: 'text', WebkitUserSelect: 'text' }}>
            <MemoizedMarkdown content={content} />
          </div>
        </div>
      ) : category === 'html' && viewMode === 'rendered' ? (
        // Render HTML in a sandboxed <iframe> — NOT via
        // dangerouslySetInnerHTML. `srcDoc` puts the document in a
        // null-origin sandbox: with `allow-scripts` (and crucially
        // WITHOUT `allow-same-origin`) the page's scripts can run so a
        // dashboard stays interactive, but they cannot reach the K2
        // app, its cookies/localStorage, or the filesystem. White
        // background so dark-themed app chrome doesn't bleed into
        // light HTML documents.
        <div className="flex-1 overflow-hidden bg-white" ref={contentRef}>
          <iframe
            title={fileName}
            srcDoc={content}
            sandbox="allow-scripts"
            className="w-full h-full border-0 bg-white"
          />
        </div>
      ) : (
        <>
          <div className="flex-1 overflow-hidden" ref={editorContainerRef}>
            <Suspense fallback={<div className="p-4 text-xs text-[var(--color-text-muted)]">Loading editor…</div>}>
              <CodeEditor
                code={editedContent ?? content}
                filePath={filePath}
                onSave={saveFile}
                onChange={(newContent) => setEditedContent(newContent)}
                onCursorChange={(line, col, selections) => setCursorInfo({ line, col, selections })}
                initialScrollTop={initialScrollTop}
                initialCursorPos={initialCursorPos}
                onPersistState={paneGroupId ? ({ scrollTop, cursorPos }) => {
                  setFileViewerState(tabId, paneGroupId, paneId, { scrollTop, cursorPos })
                } : undefined}
              />
            </Suspense>
          </div>
          {/* Status bar */}
          <div className="flex items-center gap-3 border-t border-[var(--color-border)] bg-[#111111] px-3 py-0.5 flex-shrink-0 text-[10px] text-[var(--color-text-muted)] font-mono">
            <span>Ln {cursorInfo.line}, Col {cursorInfo.col}</span>
            {cursorInfo.selections > 1 && <span>{cursorInfo.selections} selections</span>}
            <div className="flex-1" />
            <span>{getLanguageName(filePath)}</span>
            <span>UTF-8</span>
          </div>
        </>
      )}

      {/* Filepath bar — shown for non-editor views (preview, image, markdown) */}
      {viewMode !== 'edit' && (
        <div className="flex items-center border-t border-[var(--color-border)] bg-[#111111] px-3 py-0.5 flex-shrink-0">
          <span className="text-[10px] text-[var(--color-text-muted)] font-mono truncate" title={filePath}>
            {filePath}
          </span>
        </div>
      )}
    </div>
  )
}
