import { useState, useCallback, useMemo, useRef, useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { beginFileDrag, wasDropConsumed } from '@/lib/file-drag'
import { showContextMenu } from '@/lib/context-menu'
import { useFileTreeStore } from '@/stores/filetree'
import { useSettingsStore, getEffectiveKeybinding } from '@/stores/settings'
import { useTabsStore } from '@/stores/tabs'
import { useToastStore } from '@/stores/toast'
import { useFileSelectionStore } from '@/stores/file-selection'
import { useFileClipboardStore } from '@/stores/file-clipboard'
import { useFileUndoStore } from '@/stores/file-undo'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useConnectHostStore } from '@/stores/connect-host'
import { executeRemoteDrop } from '@/lib/handle-remote-drop'

// ── Types ────────────────────────────────────────────────────────────

interface FileEntry {
  name: string
  path: string
  isDirectory: boolean
  size: number
  modifiedAt: number
}

interface FileTreeProps {
  rootPath: string
  onNavigate?: (path: string) => void
}

// ── Helpers ──────────────────────────────────────────────────────────

function parentDir(path: string): string {
  const idx = path.lastIndexOf('/')
  return idx > 0 ? path.slice(0, idx) : '/'
}

/**
 * Check if a directory's subtree contains any entry matching the search query.
 * Only traverses already-cached directories (won't load new ones).
 * Used to keep a directory visible if any of its loaded descendants match.
 * Respects the showHiddenFiles toggle — hidden files don't count as matches.
 */
function subtreeHasMatch(
  dirPath: string,
  query: string,
  cache: Map<string, FileEntry[]>,
  showHiddenFiles: boolean = true
): boolean {
  const entries = cache.get(dirPath)
  if (!entries) return false
  for (const entry of entries) {
    if (!showHiddenFiles && entry.name.startsWith('.')) continue
    if (entry.name.toLowerCase().includes(query)) return true
    if (entry.isDirectory && subtreeHasMatch(entry.path, query, cache, showHiddenFiles)) return true
  }
  return false
}

/** POSIX-style dirname — returns the parent path, or '' if there is none. */
function parentPath(p: string): string {
  const idx = p.lastIndexOf('/')
  if (idx <= 0) return ''
  return p.slice(0, idx)
}

/** Collect all visible paths in tree order for shift-click range selection. */
function collectVisiblePaths(
  rootPath: string,
  cache: Map<string, FileEntry[]>,
  expandedDirs: Set<string>
): string[] {
  const result: string[] = []
  function walk(dirPath: string): void {
    const entries = cache.get(dirPath)
    if (!entries) return
    for (const entry of entries) {
      result.push(entry.path)
      if (entry.isDirectory && expandedDirs.has(entry.path)) {
        walk(entry.path)
      }
    }
  }
  walk(rootPath)
  return result
}

// ── Icons ────────────────────────────────────────────────────────────

function ChevronIcon({ expanded }: { expanded: boolean }): React.JSX.Element {
  return (
    <svg
      className={`w-3 h-3 flex-shrink-0 text-[var(--color-text-muted)] transition-transform duration-100 ${
        expanded ? 'rotate-90' : ''
      }`}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M9 5l7 7-7 7" />
    </svg>
  )
}

function FolderIcon({ open }: { open: boolean }): React.JSX.Element {
  if (open) {
    return (
      <svg
        className="w-4 h-4 flex-shrink-0"
        viewBox="0 0 24 24"
        fill="#EAB308"
        stroke="#CA8A04"
        strokeWidth={1}
      >
        <path d="M5 4h4l2 2h8a1 1 0 011 1v1H4V5a1 1 0 011-1z" />
        <path d="M3.5 9h17l-1.5 11H5L3.5 9z" />
      </svg>
    )
  }
  return (
    <svg
      className="w-4 h-4 flex-shrink-0"
      viewBox="0 0 24 24"
      fill="#EAB308"
      stroke="#CA8A04"
      strokeWidth={1}
    >
      <path d="M5 4h4l2 2h8a1 1 0 011 1v12a1 1 0 01-1 1H5a1 1 0 01-1-1V5a1 1 0 011-1z" />
    </svg>
  )
}

// ── File type icon colors ────────────────────────────────────────────

const FILE_ICON_COLORS: Record<string, string> = {
  // Web
  ts: '#3178C6', tsx: '#3178C6', js: '#F7DF1E', jsx: '#F7DF1E',
  html: '#E34F26', css: '#1572B6', scss: '#CC6699', less: '#1D365D',
  vue: '#4FC08D', svelte: '#FF3E00',
  // Data / config
  json: '#A8B9CC', yaml: '#CB171E', yml: '#CB171E', toml: '#9C4121',
  xml: '#F16529', csv: '#237346',
  // Rust / systems
  rs: '#DEA584', go: '#00ADD8', c: '#A8B9CC', cpp: '#659AD2', h: '#A8B9CC',
  zig: '#F7A41D', swift: '#F05138',
  // Scripting
  py: '#3776AB', rb: '#CC342D', lua: '#000080', sh: '#4EAA25', bash: '#4EAA25',
  zsh: '#4EAA25', fish: '#4EAA25', ps1: '#012456',
  // JVM
  java: '#B07219', kt: '#A97BFF', scala: '#DC322F',
  // Docs
  md: '#519ABA', mdx: '#519ABA', txt: '#9CA3AF', pdf: '#FF0000',
  // Images
  png: '#8B5CF6', jpg: '#8B5CF6', jpeg: '#8B5CF6', gif: '#8B5CF6',
  svg: '#FFB13B', webp: '#8B5CF6', ico: '#8B5CF6',
  // Config files
  lock: '#6B7280', env: '#ECD53F',
  // Docker / infra
  dockerfile: '#2496ED',
}

// Special full-name matches
const FILE_NAME_COLORS: Record<string, string> = {
  'Dockerfile': '#2496ED', 'Makefile': '#6B7280', 'Cargo.toml': '#DEA584',
  'package.json': '#CB3837', 'tsconfig.json': '#3178C6',
  '.gitignore': '#F05032', '.env': '#ECD53F', '.env.local': '#ECD53F',
}

function getFileColor(name: string): string {
  if (FILE_NAME_COLORS[name]) return FILE_NAME_COLORS[name]
  const ext = name.includes('.') ? name.split('.').pop()?.toLowerCase() : ''
  if (ext && FILE_ICON_COLORS[ext]) return FILE_ICON_COLORS[ext]
  return '#9CA3AF'
}

function FileIcon({ name }: { name?: string }): React.JSX.Element {
  const color = name ? getFileColor(name) : '#9CA3AF'
  return (
    <svg
      className="w-4 h-4 flex-shrink-0"
      viewBox="0 0 24 24"
      fill="none"
      stroke={color}
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8l-6-6z" />
      <path d="M14 2v6h6" />
    </svg>
  )
}

// ── Inline name editor ──────────────────────────────────────────────

function InlineNameEditor({
  initialValue,
  depth,
  icon,
  onConfirm,
  onCancel,
  selectStem
}: {
  initialValue: string
  depth: number
  icon: React.ReactNode
  onConfirm: (name: string) => void
  onCancel: () => void
  selectStem?: boolean
}): React.JSX.Element {
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const el = inputRef.current
    if (!el) return
    el.focus()
    if (selectStem && initialValue.includes('.')) {
      el.setSelectionRange(0, initialValue.lastIndexOf('.'))
    } else {
      el.select()
    }
  }, [initialValue, selectStem])

  const handleKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key === 'Enter') {
      e.preventDefault()
      const value = inputRef.current?.value.trim() || ''
      if (value) onConfirm(value)
      else onCancel()
    } else if (e.key === 'Escape') {
      e.preventDefault()
      onCancel()
    }
  }

  return (
    <div
      className="w-full flex items-center gap-1 py-[2px]"
      style={{ paddingLeft: depth * 16 + 8 }}
    >
      <span className="w-3 flex-shrink-0" />
      {icon}
      <input
        ref={inputRef}
        type="text"
        defaultValue={initialValue}
        onKeyDown={handleKeyDown}
        onBlur={() => {
          const value = inputRef.current?.value.trim() || ''
          if (value && value !== initialValue) onConfirm(value)
          else onCancel()
        }}
        className="flex-1 min-w-0 px-1 py-0 text-[13px] leading-tight bg-[var(--color-bg-primary)] border border-[var(--color-accent)] text-[var(--color-text-primary)] outline-none"
      />
    </div>
  )
}

// ── TreeItem ─────────────────────────────────────────────────────────

interface TreeItemProps {
  entry: FileEntry
  depth: number
  cache: Map<string, FileEntry[]>
  expandedDirs: Set<string>
  loadingDirs: Set<string>
  errorDirs: Map<string, string>
  selectedPaths: Record<string, true>
  cutPaths: Set<string>
  onToggleDir: (path: string) => void
  onItemClick: (entry: FileEntry, e: React.MouseEvent) => void
  onContextMenu: (e: React.MouseEvent, entry: FileEntry) => void
  onDragOutStart: (entry: FileEntry, e: React.MouseEvent) => void
  searchQuery: string
  dropTarget: string | null
  renamingPath: string | null
  onRenameConfirm: (oldPath: string, newName: string) => void
  onRenameCancel: () => void
  newEntryState: { parentPath: string; isDirectory: boolean } | null
  onNewEntryConfirm: (parentPath: string, name: string, isDirectory: boolean) => void
  onNewEntryCancel: () => void
}

function TreeItem(props: TreeItemProps): React.JSX.Element | null {
  const showHiddenFiles = useFileTreeStore((s) => s.showHiddenFiles)
  const {
    entry, depth, cache, expandedDirs, loadingDirs, errorDirs,
    selectedPaths, cutPaths,
    onToggleDir, onItemClick, onContextMenu, onDragOutStart,
    searchQuery, dropTarget, renamingPath, onRenameConfirm, onRenameCancel,
    newEntryState, onNewEntryConfirm, onNewEntryCancel
  } = props

  const isExpanded = expandedDirs.has(entry.path)
  const isLoading = loadingDirs.has(entry.path)
  const error = errorDirs.get(entry.path)
  const children = cache.get(entry.path)
  const isDropTarget = dropTarget === entry.path
  const isRenaming = renamingPath === entry.path
  const isSelected = !!selectedPaths[entry.path]
  const isCut = cutPaths.has(entry.path)
  const showNewEntry = newEntryState && newEntryState.parentPath === entry.path && entry.isDirectory

  const filteredChildren = useMemo(() => {
    if (!children) return null
    // Filter hidden files first (unless enabled)
    let list = children
    if (!showHiddenFiles) {
      list = list.filter((c) => !c.name.startsWith('.'))
    }
    if (!searchQuery) return list
    const q = searchQuery.toLowerCase()
    return list.filter((child) => {
      if (child.isDirectory) {
        if (child.name.toLowerCase().includes(q)) return true
        if (expandedDirs.has(child.path)) return true
        if (subtreeHasMatch(child.path, q, cache, showHiddenFiles)) return true
        return false
      }
      return child.name.toLowerCase().includes(q)
    })
  }, [children, searchQuery, expandedDirs, cache, showHiddenFiles])

  const handleClick = useCallback(
    (e: React.MouseEvent) => {
      if (entry.isDirectory && !e.metaKey && !e.shiftKey) {
        onToggleDir(entry.path)
      }
      onItemClick(entry, e)
    },
    [entry, onToggleDir, onItemClick]
  )

  const handleContextMenu = useCallback(
    (e: React.MouseEvent) => {
      onContextMenu(e, entry)
    },
    [entry, onContextMenu]
  )

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      if (e.button !== 0) return
      if ((e.target as HTMLElement).closest('button[data-close]')) return
      if (e.metaKey || e.shiftKey) return // Don't start drag on selection clicks
      onDragOutStart(entry, e)
    },
    [entry, onDragOutStart]
  )

  const childProps = {
    cache, expandedDirs, loadingDirs, errorDirs, selectedPaths, cutPaths,
    onToggleDir, onItemClick, onContextMenu, onDragOutStart,
    searchQuery, dropTarget, renamingPath, onRenameConfirm, onRenameCancel,
    newEntryState, onNewEntryConfirm, onNewEntryCancel
  }

  if (isRenaming) {
    return (
      <div>
        <InlineNameEditor
          initialValue={entry.name}
          depth={depth}
          icon={entry.isDirectory ? <FolderIcon open={isExpanded} /> : <FileIcon name={entry.name} />}
          onConfirm={(newName) => onRenameConfirm(entry.path, newName)}
          onCancel={onRenameCancel}
          selectStem={!entry.isDirectory}
        />
        {entry.isDirectory && isExpanded && (
          <div>
            {filteredChildren?.map((child) => (
              <TreeItem key={child.path} entry={child} depth={depth + 1} {...childProps} />
            ))}
          </div>
        )}
      </div>
    )
  }

  const selectionClass = isSelected
    ? 'bg-[var(--color-accent)]/15 ring-1 ring-inset ring-[var(--color-accent)]/40'
    : isDropTarget
      ? 'bg-[var(--color-accent)]/10 ring-1 ring-inset ring-[var(--color-accent)]'
      : ''

  return (
    <div>
      <button
        className={`w-full flex items-center gap-1 py-[3px] text-left text-[13px] leading-tight transition-colors hover:bg-white/[0.06] group ${selectionClass}`}
        style={{ paddingLeft: depth * 16 + 8, opacity: isCut ? 0.5 : 1 }}
        data-path={entry.path}
        data-is-directory={entry.isDirectory ? 'true' : 'false'}
        onClick={handleClick}
        onContextMenu={handleContextMenu}
        onMouseDown={handleMouseDown}
      >
        <span className="w-3 flex-shrink-0 flex items-center justify-center">
          {entry.isDirectory ? <ChevronIcon expanded={isExpanded} /> : null}
        </span>
        {entry.isDirectory ? <FolderIcon open={isExpanded} /> : <FileIcon name={entry.name} />}
        <span className="truncate text-[var(--color-text-secondary)] group-hover:text-[var(--color-text-primary)]">
          {entry.name}
        </span>
      </button>

      {entry.isDirectory && isExpanded && (
        <div>
          {isLoading && (
            <div
              className="py-1 text-[11px] text-[var(--color-text-muted)] italic"
              style={{ paddingLeft: (depth + 1) * 16 + 8 }}
            >
              Loading...
            </div>
          )}
          {error && (
            <div
              className="py-1 text-[11px] text-red-400 italic"
              style={{ paddingLeft: (depth + 1) * 16 + 8 }}
            >
              {error}
            </div>
          )}
          {showNewEntry && (
            <InlineNameEditor
              initialValue=""
              depth={depth + 1}
              icon={newEntryState.isDirectory ? <FolderIcon open={false} /> : <FileIcon />}
              onConfirm={(name) => onNewEntryConfirm(entry.path, name, newEntryState.isDirectory)}
              onCancel={onNewEntryCancel}
            />
          )}
          {filteredChildren?.map((child) => (
            <TreeItem key={child.path} entry={child} depth={depth + 1} {...childProps} />
          ))}
          {filteredChildren && filteredChildren.length === 0 && !isLoading && !error && !showNewEntry && (
            <div
              className="py-1 text-[11px] text-[var(--color-text-muted)] italic"
              style={{ paddingLeft: (depth + 1) * 16 + 8 }}
            >
              {searchQuery ? 'No matches' : 'Empty'}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

// ── FileTree ─────────────────────────────────────────────────────────

export default function FileTree({ rootPath }: FileTreeProps): React.JSX.Element {
  const searchQuery = useFileTreeStore((s) => s.searchQuery)
  const setSearchQuery = useFileTreeStore((s) => s.setSearchQuery)
  const showHiddenFiles = useFileTreeStore((s) => s.showHiddenFiles)
  const toggleHiddenFiles = useFileTreeStore((s) => s.toggleHiddenFiles)

  const [cache, setCache] = useState<Map<string, FileEntry[]>>(new Map())
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set([rootPath]))
  // Dirs auto-expanded because the user searched for something inside
  // them. Reverted when the search clears — unless the user opened a
  // file under them (in which case we remove from this set so the
  // expansion sticks) or manually toggled the folder.
  const searchAutoExpandedRef = useRef<Set<string>>(new Set())
  const [loadingDirs, setLoadingDirs] = useState<Set<string>>(new Set())
  const [errorDirs, setErrorDirs] = useState<Map<string, string>>(new Map())
  const [dropTarget, setDropTarget] = useState<string | null>(null)
  const [isDragOver, setIsDragOverState] = useState(false)
  const isDragOverRef = useRef(false)
  const setIsDragOver = useCallback((v: boolean) => {
    isDragOverRef.current = v
    setIsDragOverState(v)
  }, [])
  const [renamingPath, setRenamingPath] = useState<string | null>(null)
  const [newEntryState, setNewEntryState] = useState<{ parentPath: string; isDirectory: boolean } | null>(null)

  // Store subscriptions
  const selectedPaths = useFileSelectionStore((s) => s.selectedPaths)
  const clipboardMode = useFileClipboardStore((s) => s.mode)
  const clipboardPaths = useFileClipboardStore((s) => s.paths)

  // Compute cut paths for dimming
  const cutPaths = useMemo(() => {
    if (clipboardMode !== 'cut') return new Set<string>()
    return new Set(clipboardPaths)
  }, [clipboardMode, clipboardPaths])

  // Track Option key for copy vs move
  const optionKeyRef = useRef(false)
  useEffect(() => {
    const down = (e: KeyboardEvent): void => { if (e.key === 'Alt') optionKeyRef.current = true }
    const up = (e: KeyboardEvent): void => { if (e.key === 'Alt') optionKeyRef.current = false }
    window.addEventListener('keydown', down)
    window.addEventListener('keyup', up)
    return () => { window.removeEventListener('keydown', down); window.removeEventListener('keyup', up) }
  }, [])

  // Toggle hidden files hotkey — default CMD+Shift+. (matches macOS Finder), user-rebindable
  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      const { keybindings } = useSettingsStore.getState()
      const combo = getEffectiveKeybinding(keybindings, 'toggleHiddenFiles')
      if (!combo) return
      const parts = combo.split('+')
      const key = parts[parts.length - 1]
      const wantMeta = parts.includes('Meta')
      const wantShift = parts.includes('Shift')
      const wantAlt = parts.includes('Alt')
      const wantCtrl = parts.includes('Ctrl')
      if (e.metaKey === wantMeta && e.shiftKey === wantShift && e.altKey === wantAlt && e.ctrlKey === wantCtrl && e.key === key) {
        e.preventDefault()
        toggleHiddenFiles()
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [toggleHiddenFiles])

  const treeRef = useRef<HTMLDivElement>(null)

  // Track the root path so we can reset when it changes
  const prevRootPath = useRef(rootPath)
  if (prevRootPath.current !== rootPath) {
    prevRootPath.current = rootPath
    setCache(new Map())
    setExpandedDirs(new Set([rootPath]))
    setLoadingDirs(new Set())
    setErrorDirs(new Map())
    // 0.37.11 — clearSelection writes to a SEPARATE zustand store,
    // which triggers its subscribers' re-renders mid-render of
    // FileTree. React (correctly) flags that as "setState in
    // render of a different component." Defer to a microtask so
    // it lands after the current render commits.
    queueMicrotask(() => useFileSelectionStore.getState().clearSelection())
  }

  // ── Env files section ──────────────────────────────────────────────
  const [envFiles, setEnvFiles] = useState<FileEntry[]>([])
  const [envCollapsed, setEnvCollapsed] = useState(false)

  const loadEnvFiles = useCallback(async () => {
    try {
      // Search root + common subdirectories for .env* files
      const searchPaths = [rootPath]
      // Also check common config locations
      const rootEntries = await daemonCliGet<FileEntry[]>('fs/read-dir', { path: rootPath, show_hidden: true })
      for (const e of rootEntries) {
        if (e.isDirectory && !e.name.startsWith('.') && !['node_modules', 'target', 'dist', 'build', '.git', 'vendor'].includes(e.name)) {
          searchPaths.push(e.path)
        }
      }

      const allEnvFiles: FileEntry[] = []
      for (const dir of searchPaths) {
        try {
          const entries = await daemonCliGet<FileEntry[]>('fs/read-dir', { path: dir, show_hidden: true })
          for (const e of entries) {
            if (!e.isDirectory && (e.name.startsWith('.env') || e.name === 'env' || e.name.endsWith('.env'))) {
              allEnvFiles.push(e)
            }
          }
        } catch { /* skip inaccessible dirs */ }
      }

      // Deduplicate by path and sort
      const seen = new Set<string>()
      const unique = allEnvFiles.filter((e) => {
        if (seen.has(e.path)) return false
        seen.add(e.path)
        return true
      })
      unique.sort((a, b) => a.path.localeCompare(b.path))
      setEnvFiles(unique)
    } catch {
      setEnvFiles([])
    }
  }, [rootPath])

  // ── AI Config section ─────────────────────────────────────────────
  // Shows hidden config folders/files for AI tools (Claude, Cursor, Copilot, etc.)
  const AI_CONFIG_PATTERNS: { name: string; match: (e: FileEntry) => boolean }[] = [
    { name: '.claude', match: (e) => e.isDirectory && e.name === '.claude' },
    { name: '.cursor', match: (e) => e.isDirectory && e.name === '.cursor' },
    { name: '.cursorrules', match: (e) => !e.isDirectory && e.name === '.cursorrules' },
    { name: '.github/copilot', match: (e) => e.isDirectory && e.name === '.github' },
    { name: '.aider*', match: (e) => e.name.startsWith('.aider') },
    { name: '.continue', match: (e) => e.isDirectory && e.name === '.continue' },
    { name: 'CLAUDE.md', match: (e) => !e.isDirectory && e.name === 'CLAUDE.md' },
    { name: 'SKILL.md', match: (e) => !e.isDirectory && e.name === 'SKILL.md' },
    { name: 'AGENTS.md', match: (e) => !e.isDirectory && e.name === 'AGENTS.md' },
    { name: '.opencode', match: (e) => e.isDirectory && e.name === '.opencode' },
    { name: '.pi', match: (e) => e.isDirectory && e.name === '.pi' },
    { name: '.windsurfrules', match: (e) => !e.isDirectory && e.name === '.windsurfrules' },
  ]

  const [aiConfigEntries, setAiConfigEntries] = useState<FileEntry[]>([])
  const [aiConfigCollapsed, setAiConfigCollapsed] = useState(false)

  const loadAiConfig = useCallback(async () => {
    try {
      const entries = await daemonCliGet<FileEntry[]>('fs/read-dir', {
        path: rootPath,
        show_hidden: true,
      })
      const matched = entries.filter((e) =>
        AI_CONFIG_PATTERNS.some((p) => p.match(e))
      )
      setAiConfigEntries(matched)
    } catch {
      setAiConfigEntries([])
    }
  }, [rootPath])

  // Load directory contents. We always fetch with `showHidden: true` so
  // the cache contains every entry; the render-time filter
  // (filteredChildren / filteredRootEntries) hides dotfiles based on
  // the store's `showHiddenFiles` toggle. This way, flipping the eye
  // icon is a pure re-render — no re-fetch needed.
  const loadDir = useCallback(
    async (dirPath: string, force = false) => {
      if (!force && cache.has(dirPath)) return

      setLoadingDirs((prev) => new Set(prev).add(dirPath))
      setErrorDirs((prev) => {
        const next = new Map(prev)
        next.delete(dirPath)
        return next
      })

      try {
        const entries = await daemonCliGet<FileEntry[]>('fs/read-dir', { path: dirPath, show_hidden: true })
        setCache((prev) => new Map(prev).set(dirPath, entries))
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Failed to read directory'
        setErrorDirs((prev) => new Map(prev).set(dirPath, message))
      } finally {
        setLoadingDirs((prev) => {
          const next = new Set(prev)
          next.delete(dirPath)
          return next
        })
      }
    },
    [cache]
  )

  // When `showHiddenFiles` changes, any directories already in the cache
  // may have been fetched before this render rule existed (old app
  // sessions filtered at fetch time rather than at render time). Force
  // a re-fetch of every cached dir so the cache catches up to the
  // "always fetch everything" invariant.
  //
  // This is a one-time correction — subsequent toggles are cheap because
  // loadDir only refetches if called with force=true.
  const firstToggleDoneRef = useRef(false)
  useEffect(() => {
    if (!firstToggleDoneRef.current) {
      firstToggleDoneRef.current = true
      return // skip the initial mount pass
    }
    const cachedDirs = Array.from(cache.keys())
    for (const dir of cachedDirs) {
      loadDir(dir, true)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showHiddenFiles])

  // Refresh affected directories after a file operation
  const refreshDirs = useCallback(async (paths: string[]) => {
    const dirsToRefresh = new Set<string>()
    for (const p of paths) {
      dirsToRefresh.add(parentDir(p))
    }
    for (const dir of dirsToRefresh) {
      await loadDir(dir, true)
    }
  }, [loadDir])

  // Load env files on mount / root change
  useEffect(() => { loadEnvFiles() }, [loadEnvFiles])

  // Load AI config entries on mount / root change
  useEffect(() => { loadAiConfig() }, [loadAiConfig])

  // ── FS Watcher ──────────────────────────────────────────────────────
  useEffect(() => {
    // 0.37.11 — skip the watcher entirely when rootPath isn't a real
    // filesystem path. Focus windows briefly mount with rootPath="~"
    // (placeholder) before the project loads, and `fs_watch_dir`
    // rejects with "No path was found" for non-existent paths. The
    // useEffect will re-run when rootPath becomes a real path.
    if (!rootPath || rootPath === '~' || !rootPath.startsWith('/')) {
      return
    }
    // Start watching the root path
    invoke('fs_watch_dir', { path: rootPath }).catch((err) => {
      console.warn('[filetree] Failed to start watcher:', err)
    })

    // Listen for FS change events and refresh affected directories.
    // 0.32.13+ the backend emits a batched payload (Vec<FsChangePayload>)
    // per debounce window instead of one-emit-per-path. We accept either
    // shape so older backend builds still work — single objects are
    // wrapped into a one-element array.
    let unlisten: (() => void) | null = null
    listen<Array<{ path: string; kind: string }> | { path: string; kind: string }>('fs://change', (event) => {
      const batch = Array.isArray(event.payload) ? event.payload : [event.payload]
      // Collect unique parent dirs from the batch so we reload each once
      // even when many paths share a directory.
      const dirsToRefresh = new Set<string>()
      let refreshEnv = false
      let refreshAiConfig = false
      for (const change of batch) {
        const dir = parentDir(change.path)
        dirsToRefresh.add(dir)
        const changedName = change.path.split('/').pop() || ''
        if (dir === rootPath && changedName.startsWith('.env')) {
          refreshEnv = true
        }
        if (dir === rootPath && AI_CONFIG_PATTERNS.some((p) => p.match({ name: changedName, path: change.path, isDirectory: false, size: 0, modifiedAt: 0 }) || p.match({ name: changedName, path: change.path, isDirectory: true, size: 0, modifiedAt: 0 }))) {
          refreshAiConfig = true
        }
      }
      setCache((prev) => {
        for (const dir of dirsToRefresh) {
          if (prev.has(dir)) {
            loadDir(dir, true)
          }
        }
        return prev
      })
      if (refreshEnv) loadEnvFiles()
      if (refreshAiConfig) loadAiConfig()
    }).then((fn) => { unlisten = fn })

    return () => {
      unlisten?.()
      invoke('fs_unwatch_dir', { path: rootPath }).catch((e) => console.warn('[file-tree]', e))
    }
  }, [rootPath, loadDir, loadEnvFiles, loadAiConfig])

  // ── Drop-in from Finder (Tauri events) ─────────────────────────────
  useEffect(() => {
    const unlisteners: Array<() => void> = []

    listen<{ paths: string[]; position: { x: number; y: number } }>('tauri://drag-drop', async (event) => {
      setIsDragOver(false)
      setDropTarget(null)

      const { paths, position } = event.payload
      if (!paths || paths.length === 0) return

      // Check if the drop position is inside a terminal — if so, skip entirely
      const dropEl = document.elementFromPoint(position.x, position.y)
      if (dropEl && (dropEl as HTMLElement).closest?.('[data-terminal-id]')) return

      // Only handle drops that land inside the file tree panel
      if (!treeRef.current) return
      const treeRect = treeRef.current.getBoundingClientRect()
      const inTree = (
        position.x >= treeRect.left && position.x <= treeRect.right &&
        position.y >= treeRect.top && position.y <= treeRect.bottom
      )
      if (!inTree) return

      let targetFolder = rootPath
      const el = document.elementFromPoint(position.x, position.y)
      if (el) {
        const btn = (el as HTMLElement).closest('[data-path]') as HTMLElement | null
        if (btn) {
          const isDir = btn.dataset.isDirectory === 'true'
          const path = btn.dataset.path
          if (path) {
            targetFolder = isDir ? path : parentDir(path)
          }
        }
      }

      // REMOTE host (K2 Connect): the dropped `paths` are LOCAL paths with
      // no bytes on the daemon, so fs/move|copy (which expect daemon-side
      // paths) can't work. Upload the bytes INTO the target folder instead
      // (the "folder" case of the shared drop router). No path injection —
      // a file-tree drop is a plain copy-into-folder, like the local case.
      if (useConnectHostStore.getState().activeHost !== 'local') {
        await executeRemoteDrop(paths, { kind: 'folder', path: targetFolder }, {})
        await loadDir(targetFolder, true)
        return
      }

      const toast = useToastStore.getState()
      const undo = useFileUndoStore.getState()
      const isCopy = optionKeyRef.current
      const action = isCopy ? 'Copied' : 'Moved'

      try {
        if (isCopy) {
          await daemonCliPost('fs/copy', { sources: paths, destination: targetFolder })
          undo.push({ type: 'copy', createdPaths: paths.map(p => `${targetFolder}/${p.split('/').pop()}`) })
        } else {
          await daemonCliPost('fs/move', { sources: paths, destination: targetFolder })
          undo.push({
            type: 'move',
            items: paths.map(p => ({ oldPath: p, newPath: `${targetFolder}/${p.split('/').pop()}` }))
          })
        }
        toast.addToast(`${action} ${paths.length} item${paths.length > 1 ? 's' : ''}`, 'success')
        await loadDir(targetFolder, true)
      } catch (err) {
        toast.addToast(`Failed: ${err}`, 'error')
      }
    }).then((fn) => unlisteners.push(fn))

    listen('tauri://drag-enter', () => {
      // Don't set isDragOver here — let drag-over handle it based on position,
      // so drops on the terminal don't get treated as file tree drops.
    }).then((fn) => unlisteners.push(fn))

    listen<{ position: { x: number; y: number } }>('tauri://drag-over', (event) => {
      const { position } = event.payload
      // Only show drop targets when hovering over the file tree panel
      if (!treeRef.current) {
        setIsDragOver(false)
        setDropTarget(null)
        return
      }
      const rect = treeRef.current.getBoundingClientRect()
      if (
        position.x < rect.left || position.x > rect.right ||
        position.y < rect.top || position.y > rect.bottom
      ) {
        setIsDragOver(false)
        setDropTarget(null)
        return
      }
      setIsDragOver(true)
      const el = document.elementFromPoint(position.x, position.y)
      if (el) {
        const btn = (el as HTMLElement).closest('[data-path]') as HTMLElement | null
        if (btn && btn.dataset.isDirectory === 'true' && btn.dataset.path) {
          setDropTarget(btn.dataset.path)
          return
        }
      }
      setDropTarget(rootPath)
    }).then((fn) => unlisteners.push(fn))

    listen('tauri://drag-leave', () => {
      setIsDragOver(false)
      setDropTarget(null)
    }).then((fn) => unlisteners.push(fn))

    return () => {
      unlisteners.forEach((fn) => fn())
    }
  }, [rootPath, loadDir])

  // ── Internal drag state (for highlighting folders during drag) ─────
  const [internalDropTarget, setInternalDropTarget] = useState<string | null>(null)

  // ── Drag-out: in-window (terminal drop) + folder drop + Finder (OS handoff)
  const handleDragOutStart = useCallback((entry: FileEntry, e: React.MouseEvent) => {
    const startX = e.clientX
    const startY = e.clientY
    let started = false

    // Drag all selected items if the dragged item is selected, otherwise just drag the one
    const selection = useFileSelectionStore.getState()
    const paths = selection.isSelected(entry.path)
      ? selection.getSelectedPaths()
      : [entry.path]

    const handleMouseMove = (ev: MouseEvent): void => {
      if (!started && (Math.abs(ev.clientX - startX) > 5 || Math.abs(ev.clientY - startY) > 5)) {
        started = true
        beginFileDrag(paths, ev.clientX, ev.clientY, {
          onDragOver: (dirPath) => {
            // Don't highlight if hovering over one of the dragged items' own directories
            if (dirPath && paths.some(p => p === dirPath || dirPath.startsWith(p + '/'))) {
              setInternalDropTarget(null)
            } else {
              setInternalDropTarget(dirPath)
            }
          },
          onDrop: (dirPath) => {
            // Don't allow dropping into self or a child of a dragged folder
            if (paths.some(p => p === dirPath || dirPath.startsWith(p + '/'))) return false
            // Don't allow "moving" if already in the target directory
            if (paths.every(p => parentDir(p) === dirPath)) return false

            const toast = useToastStore.getState()
            const undo = useFileUndoStore.getState()
            const isCopy = optionKeyRef.current

            const doMove = async (): Promise<void> => {
              try {
                if (isCopy) {
                  await daemonCliPost('fs/copy', { sources: paths, destination: dirPath })
                  undo.push({
                    type: 'copy',
                    createdPaths: paths.map(p => `${dirPath}/${p.split('/').pop()}`)
                  })
                  toast.addToast(`Copied ${paths.length} item${paths.length > 1 ? 's' : ''}`, 'success')
                } else {
                  await daemonCliPost('fs/move', { sources: paths, destination: dirPath })
                  undo.push({
                    type: 'move',
                    items: paths.map(p => ({
                      oldPath: p,
                      newPath: `${dirPath}/${p.split('/').pop()}`
                    }))
                  })
                  toast.addToast(`Moved ${paths.length} item${paths.length > 1 ? 's' : ''}`, 'success')
                }
                // Refresh affected directories
                const dirsToRefresh = new Set<string>()
                dirsToRefresh.add(dirPath)
                for (const p of paths) dirsToRefresh.add(parentDir(p))
                for (const dir of dirsToRefresh) await loadDir(dir, true)
              } catch (err) {
                toast.addToast(`${isCopy ? 'Copy' : 'Move'} failed: ${err}`, 'error')
              }
            }
            doMove()
            return true
          },
          onDragEnd: () => {
            setInternalDropTarget(null)
          }
        })
        document.removeEventListener('mousemove', handleMouseMove)
        document.removeEventListener('mouseup', handleMouseUp)
      }
    }

    const handleMouseUp = (): void => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [loadDir])

  // Toggle directory expand/collapse. A manual toggle overrides the
  // search auto-expand tracking — if the user explicitly collapsed (or
  // re-expanded) a folder, we stop treating it as "search-expanded".
  const handleToggleDir = useCallback(
    (dirPath: string) => {
      searchAutoExpandedRef.current.delete(dirPath)
      setExpandedDirs((prev) => {
        const next = new Set(prev)
        if (next.has(dirPath)) {
          next.delete(dirPath)
        } else {
          next.add(dirPath)
          loadDir(dirPath)
        }
        return next
      })
    },
    [loadDir]
  )

  // When the user opens a file that lives inside a search-auto-expanded
  // folder, promote those ancestor folders to "user-expanded" so they
  // don't collapse when the search is cleared.
  const commitAncestorsToUser = useCallback((filePath: string) => {
    if (searchAutoExpandedRef.current.size === 0) return
    let p = parentPath(filePath)
    while (p && p.startsWith(rootPath)) {
      searchAutoExpandedRef.current.delete(p)
      if (p === rootPath) break
      p = parentPath(p)
    }
  }, [rootPath])

  // ── Selection + Click ──────────────────────────────────────────────
  const handleItemClick = useCallback(
    (entry: FileEntry, e: React.MouseEvent) => {
      const selection = useFileSelectionStore.getState()

      if (e.metaKey) {
        // Cmd+click: toggle selection
        selection.toggleSelect(entry.path)
      } else if (e.shiftKey) {
        // Shift+click: range select
        const allPaths = collectVisiblePaths(rootPath, cache, expandedDirs)
        selection.rangeSelect(entry.path, allPaths)
      } else {
        // Plain click: single select
        selection.select(entry.path)
        // Open file on plain click — dedup: switch to existing tab if already open
        if (!entry.isDirectory) {
          useTabsStore.getState().openFileAsTab(entry.path)
          commitAncestorsToUser(entry.path)
        }
      }
    },
    [rootPath, cache, expandedDirs, commitAncestorsToUser]
  )

  // ── Rename ──────────────────────────────────────────────────────────
  const handleRenameConfirm = useCallback(async (oldPath: string, newName: string) => {
    const toast = useToastStore.getState()
    const undo = useFileUndoStore.getState()
    try {
      const r = await daemonCliPost<{ path: string }>('fs/rename', { old_path: oldPath, new_name: newName })
      const newPath = r.path
      toast.addToast(`Renamed to ${newName}`, 'success')
      undo.push({ type: 'rename', oldPath, newPath })
      await refreshDirs([oldPath])
    } catch (err) {
      toast.addToast(`Rename failed: ${err}`, 'error')
    }
    setRenamingPath(null)
  }, [refreshDirs])

  const handleRenameCancel = useCallback(() => {
    setRenamingPath(null)
  }, [])

  // ── New file / folder ─────────────────────────────────────────────
  const handleNewEntryConfirm = useCallback(async (parentPath: string, name: string, isDirectory: boolean) => {
    const toast = useToastStore.getState()
    const undo = useFileUndoStore.getState()
    const fullPath = `${parentPath}/${name}`
    try {
      await daemonCliPost('fs/create', { path: fullPath, is_directory: isDirectory })
      toast.addToast(`Created ${name}`, 'success')
      undo.push({ type: 'create', path: fullPath })
      await loadDir(parentPath, true)
      if (!isDirectory) {
        useTabsStore.getState().openFileInNewTab(fullPath)
      }
    } catch (err) {
      toast.addToast(`Create failed: ${err}`, 'error')
    }
    setNewEntryState(null)
  }, [loadDir])

  const handleNewEntryCancel = useCallback(() => {
    setNewEntryState(null)
  }, [])

  // ── Delete (with confirmation) ────────────────────────────────────
  const handleDelete = useCallback(async (paths: string[]) => {
    if (paths.length === 0) return
    const toast = useToastStore.getState()
    const undo = useFileUndoStore.getState()
    const confirm = useConfirmDialogStore.getState().confirm

    const names = paths.map(p => p.split('/').pop() || p)
    const message = paths.length === 1
      ? `Move "${names[0]}" to Trash?`
      : `Move ${paths.length} items to Trash?\n${names.slice(0, 5).join(', ')}${names.length > 5 ? `, and ${names.length - 5} more` : ''}`

    const confirmed = await confirm({
      title: 'Move to Trash',
      message,
      confirmLabel: 'Move to Trash',
      destructive: true
    })

    if (!confirmed) return

    try {
      await daemonCliPost('fs/delete', { paths })
      const label = paths.length === 1 ? `Moved ${names[0]} to Trash` : `Moved ${paths.length} items to Trash`
      toast.addToast(label, 'success')
      undo.push({ type: 'delete', paths, note: 'trashed' })
      await refreshDirs(paths)
      useFileSelectionStore.getState().clearSelection()
    } catch (err) {
      toast.addToast(`Delete failed: ${err}`, 'error')
    }
  }, [refreshDirs])

  // ── Clipboard paste ───────────────────────────────────────────────
  const handlePaste = useCallback(async (targetDir: string) => {
    const clipboard = useFileClipboardStore.getState()
    const toast = useToastStore.getState()
    const undo = useFileUndoStore.getState()

    if (!clipboard.hasPaths()) return

    try {
      if (clipboard.mode === 'copy') {
        await daemonCliPost('fs/copy', { sources: clipboard.paths, destination: targetDir })
        undo.push({
          type: 'copy',
          createdPaths: clipboard.paths.map(p => `${targetDir}/${p.split('/').pop()}`)
        })
        toast.addToast(`Pasted ${clipboard.paths.length} item(s)`, 'success')
      } else if (clipboard.mode === 'cut') {
        await daemonCliPost('fs/move', { sources: clipboard.paths, destination: targetDir })
        undo.push({
          type: 'move',
          items: clipboard.paths.map(p => ({
            oldPath: p,
            newPath: `${targetDir}/${p.split('/').pop()}`
          }))
        })
        toast.addToast(`Moved ${clipboard.paths.length} item(s)`, 'success')
        clipboard.clear()
        // Refresh source dirs
        await refreshDirs(clipboard.paths)
      }
      await loadDir(targetDir, true)
    } catch (err) {
      toast.addToast(`Paste failed: ${err}`, 'error')
    }
  }, [loadDir, refreshDirs])

  // ── Duplicate ─────────────────────────────────────────────────────
  const handleDuplicate = useCallback(async (paths: string[]) => {
    const toast = useToastStore.getState()
    const undo = useFileUndoStore.getState()
    const created: string[] = []

    for (const p of paths) {
      try {
        const r = await daemonCliPost<{ path: string }>('fs/duplicate', { path: p })
        const newPath = r.path
        created.push(newPath)
      } catch (err) {
        toast.addToast(`Duplicate failed: ${err}`, 'error')
        break
      }
    }

    if (created.length > 0) {
      undo.push({ type: 'copy', createdPaths: created })
      toast.addToast(`Duplicated ${created.length} item(s)`, 'success')
      await refreshDirs(created)
    }
  }, [refreshDirs])

  // ── Undo ──────────────────────────────────────────────────────────
  const handleUndo = useCallback(async () => {
    const undo = useFileUndoStore.getState()
    const toast = useToastStore.getState()
    const op = undo.pop()
    if (!op) return

    try {
      switch (op.type) {
        case 'create':
          await daemonCliPost('fs/delete', { paths: [op.path] })
          toast.addToast('Undid create', 'success')
          await refreshDirs([op.path])
          break
        case 'rename':
          // Rename back: extract the old name from oldPath
          const oldName = op.oldPath.split('/').pop() || ''
          await daemonCliPost('fs/rename', { old_path: op.newPath, new_name: oldName })
          toast.addToast('Undid rename', 'success')
          await refreshDirs([op.newPath])
          break
        case 'move':
          // Move items back to their original locations
          for (const item of [...op.items].reverse()) {
            const origDir = parentDir(item.oldPath)
            await daemonCliPost('fs/move', { sources: [item.newPath], destination: origDir })
          }
          toast.addToast('Undid move', 'success')
          await refreshDirs([...op.items.map(i => i.oldPath), ...op.items.map(i => i.newPath)])
          break
        case 'copy':
          // Delete the copies
          await daemonCliPost('fs/delete', { paths: op.createdPaths })
          toast.addToast('Undid copy', 'success')
          await refreshDirs(op.createdPaths)
          break
        case 'delete':
          toast.addToast('Cannot undo trash (restore from Finder)', 'info')
          break
      }
    } catch (err) {
      toast.addToast(`Undo failed: ${err}`, 'error')
    }
  }, [refreshDirs])

  // ── Keyboard shortcuts ────────────────────────────────────────────
  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      // Only handle when file tree area is focused
      if (!treeRef.current?.contains(document.activeElement) && document.activeElement !== treeRef.current) {
        return
      }

      // Don't intercept keys when an input or textarea is focused (e.g., search box, rename input)
      const tag = (document.activeElement as HTMLElement)?.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA') return

      const selection = useFileSelectionStore.getState()
      const paths = selection.getSelectedPaths()

      if (e.metaKey && e.key === 'c') {
        if (paths.length > 0) {
          e.preventDefault()
          useFileClipboardStore.getState().copy(paths)
          useToastStore.getState().addToast(`Copied ${paths.length} item(s)`, 'success')
        }
      } else if (e.metaKey && e.key === 'x') {
        if (paths.length > 0) {
          e.preventDefault()
          useFileClipboardStore.getState().cut(paths)
          useToastStore.getState().addToast(`Cut ${paths.length} item(s)`, 'success')
        }
      } else if (e.metaKey && e.key === 'v') {
        e.preventDefault()
        // Paste into the first selected directory, or its parent, or root
        let targetDir = rootPath
        if (paths.length === 1) {
          const p = paths[0]
          // Check if it's a directory (check cache)
          const parent = parentDir(p)
          const entries = cache.get(parent)
          const entry = entries?.find(en => en.path === p)
          targetDir = entry?.isDirectory ? p : parent
        }
        handlePaste(targetDir)
      } else if (e.metaKey && e.key === 'd') {
        if (paths.length > 0) {
          e.preventDefault()
          handleDuplicate(paths)
        }
      } else if (e.metaKey && e.key === 'z' && !e.shiftKey) {
        e.preventDefault()
        handleUndo()
      } else if ((e.key === 'Backspace' || e.key === 'Delete') && !e.metaKey) {
        if (paths.length > 0) {
          e.preventDefault()
          handleDelete(paths)
        }
      } else if (e.key === 'Enter' && paths.length === 1 && !e.metaKey) {
        e.preventDefault()
        setRenamingPath(paths[0])
      }
    }

    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [rootPath, cache, handlePaste, handleDuplicate, handleUndo, handleDelete])

  // Context menu
  const handleContextMenu = useCallback(async (e: React.MouseEvent, entry: FileEntry) => {
    e.preventDefault()

    // If right-clicking an unselected item, select it
    const selection = useFileSelectionStore.getState()
    if (!selection.isSelected(entry.path)) {
      selection.select(entry.path)
    }

    const paths = selection.getSelectedPaths()
    const isSingle = paths.length <= 1
    const isDir = entry.isDirectory
    const hasClipboard = useFileClipboardStore.getState().hasPaths()

    const items = [
      ...(isDir && isSingle
        ? [
            { id: 'new-file', label: 'New File' },
            { id: 'new-folder', label: 'New Folder' },
            { id: 'separator-new', label: '', type: 'separator' }
          ]
        : []),
      { id: 'copy-items', label: `Copy${!isSingle ? ` (${paths.length})` : ''}` },
      { id: 'cut-items', label: `Cut${!isSingle ? ` (${paths.length})` : ''}` },
      ...(hasClipboard
        ? [{ id: 'paste', label: 'Paste' }]
        : []),
      { id: 'separator-clip', label: '', type: 'separator' },
      ...(isSingle
        ? [{ id: 'rename', label: 'Rename' }]
        : []),
      { id: 'duplicate', label: `Duplicate${!isSingle ? ` (${paths.length})` : ''}` },
      { id: 'delete', label: `Move to Trash${!isSingle ? ` (${paths.length})` : ''}` },
      { id: 'separator-util', label: '', type: 'separator' },
      { id: 'open-finder', label: 'Open in Finder' },
      { id: 'copy-path', label: 'Copy Path' }
    ]

    const clickedId = await showContextMenu(items)

    if (clickedId === 'open-finder') {
      await daemonCliPost('fs/open-finder', { target: entry.path })
    } else if (clickedId === 'copy-path') {
      await navigator.clipboard.writeText(entry.path).catch((err) => console.warn('[file-tree] clipboard write', err))
    } else if (clickedId === 'rename') {
      setRenamingPath(entry.path)
    } else if (clickedId === 'delete') {
      await handleDelete(paths)
    } else if (clickedId === 'duplicate') {
      await handleDuplicate(paths)
    } else if (clickedId === 'copy-items') {
      useFileClipboardStore.getState().copy(paths)
      useToastStore.getState().addToast(`Copied ${paths.length} item(s)`, 'success')
    } else if (clickedId === 'cut-items') {
      useFileClipboardStore.getState().cut(paths)
      useToastStore.getState().addToast(`Cut ${paths.length} item(s)`, 'success')
    } else if (clickedId === 'paste') {
      const targetDir = isDir ? entry.path : parentDir(entry.path)
      await handlePaste(targetDir)
    } else if (clickedId === 'new-file') {
      setExpandedDirs((prev) => {
        const next = new Set(prev)
        next.add(entry.path)
        return next
      })
      await loadDir(entry.path)
      setNewEntryState({ parentPath: entry.path, isDirectory: false })
    } else if (clickedId === 'new-folder') {
      setExpandedDirs((prev) => {
        const next = new Set(prev)
        next.add(entry.path)
        return next
      })
      await loadDir(entry.path)
      setNewEntryState({ parentPath: entry.path, isDirectory: true })
    }
  }, [handleDelete, handleDuplicate, handlePaste, loadDir])

  // Load root on first expand
  const rootEntries = cache.get(rootPath)
  if (!rootEntries && !loadingDirs.has(rootPath) && !errorDirs.has(rootPath)) {
    loadDir(rootPath)
  }

  // Auto-expand folders containing search matches; revert when search
  // clears. Matches come from a backend filesystem walk so files in
  // folders the user has never opened still surface. We then load each
  // ancestor folder into the cache and add it to expandedDirs; the
  // existing render path fills in the rest.
  //
  // Folders the user had already expanded manually remain "user-owned"
  // and won't collapse on search clear. Opening a file during search
  // also promotes its ancestor chain to user-owned.
  const searchRequestIdRef = useRef(0)
  useEffect(() => {
    const trimmed = searchQuery.trim()

    if (!trimmed) {
      const toCollapse = searchAutoExpandedRef.current
      if (toCollapse.size === 0) return
      setExpandedDirs((prev) => {
        const next = new Set(prev)
        for (const p of toCollapse) next.delete(p)
        return next
      })
      searchAutoExpandedRef.current = new Set()
      return
    }

    // Debounce + race-cancel: each keystroke bumps the request id;
    // stale responses are discarded so fast typing doesn't flicker.
    const myRequestId = ++searchRequestIdRef.current
    const timeoutId = window.setTimeout(async () => {
      let matches: Array<{ path: string; name: string; isDirectory: boolean }>
      try {
        matches = await invoke<Array<{ path: string; name: string; isDirectory: boolean }>>(
          'fs_search_tree',
          { root: rootPath, query: trimmed, showHidden: showHiddenFiles, maxResults: 500 }
        )
      } catch (err) {
        console.warn('[file-tree] search failed:', err)
        return
      }
      if (myRequestId !== searchRequestIdRef.current) return

      // Collect every ancestor directory of every match, stopping at
      // the root. These are the folders we'll expand to make each
      // match visible.
      const ancestors = new Set<string>()
      for (const m of matches) {
        let p = parentPath(m.path)
        while (p && p.startsWith(rootPath)) {
          if (p !== rootPath) ancestors.add(p)
          if (p === rootPath) break
          p = parentPath(p)
        }
      }

      // Load any ancestor directories that aren't in the cache yet.
      // Must wait for all loads before toggling expanded state, or the
      // tree renders empty rows briefly.
      const toLoad: string[] = []
      for (const a of ancestors) {
        if (!cache.has(a) && !loadingDirs.has(a)) toLoad.push(a)
      }
      if (toLoad.length > 0) {
        await Promise.all(toLoad.map((p) => loadDir(p)))
        if (myRequestId !== searchRequestIdRef.current) return
      }

      setExpandedDirs((prev) => {
        const next = new Set(prev)
        const newAutoExpanded = new Set<string>()

        for (const p of searchAutoExpandedRef.current) {
          if (ancestors.has(p)) newAutoExpanded.add(p)
          else next.delete(p)
        }

        for (const a of ancestors) {
          if (!next.has(a)) {
            next.add(a)
            newAutoExpanded.add(a)
          }
        }

        searchAutoExpandedRef.current = newAutoExpanded
        return next
      })
    }, 180)

    return () => {
      window.clearTimeout(timeoutId)
    }
  }, [searchQuery, showHiddenFiles, rootPath, cache, loadingDirs, loadDir])

  // Filter root entries by search and hidden files toggle
  const filteredRootEntries = useMemo(() => {
    if (!rootEntries) return null
    let list = rootEntries
    if (!showHiddenFiles) {
      list = list.filter((e) => !e.name.startsWith('.'))
    }
    if (!searchQuery) return list
    const q = searchQuery.toLowerCase()
    return list.filter((entry) => {
      if (entry.isDirectory) {
        if (entry.name.toLowerCase().includes(q)) return true
        if (expandedDirs.has(entry.path)) return true
        if (subtreeHasMatch(entry.path, q, cache, showHiddenFiles)) return true
        return false
      }
      return entry.name.toLowerCase().includes(q)
    })
  }, [rootEntries, searchQuery, expandedDirs, cache, showHiddenFiles])

  const rootBasename = rootPath.split('/').pop() || rootPath

  // Merge Finder drag-in target with internal drag target
  const effectiveDropTarget = dropTarget ?? internalDropTarget

  const childProps = {
    cache, expandedDirs, loadingDirs, errorDirs, selectedPaths, cutPaths,
    onToggleDir: handleToggleDir,
    onItemClick: handleItemClick,
    onContextMenu: handleContextMenu,
    onDragOutStart: handleDragOutStart,
    searchQuery, dropTarget: effectiveDropTarget, renamingPath,
    onRenameConfirm: handleRenameConfirm,
    onRenameCancel: handleRenameCancel,
    newEntryState,
    onNewEntryConfirm: handleNewEntryConfirm,
    onNewEntryCancel: handleNewEntryCancel
  }

  return (
    <div ref={treeRef} className="flex flex-col h-full" tabIndex={-1} data-file-tree-panel="true">
      {/* Env files section */}
      {envFiles.length > 0 && (
        <div className="px-3 pt-2 pb-1 border-b border-[var(--color-border)]">
          <button
            className="w-full flex items-center gap-1.5 mb-1.5 text-left text-[10px] uppercase tracking-wider font-semibold text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors"
            onClick={() => setEnvCollapsed((prev) => !prev)}
          >
            <svg
              className={`w-2.5 h-2.5 flex-shrink-0 transition-transform ${envCollapsed ? '' : 'rotate-90'}`}
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2.5}
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
            </svg>
            <span className="flex-1">Environment</span>
          </button>
          {!envCollapsed && (
            <div className="flex flex-wrap gap-1.5 pb-1.5">
              {envFiles.map((entry) => (
                <button
                  key={entry.path}
                  className="inline-flex items-center gap-1.5 px-2 py-1 text-[11px] bg-white/[0.04] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-white/[0.08] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors"
                  onClick={() => {
                    useFileSelectionStore.getState().select(entry.path)
                    useTabsStore.getState().openFileAsTab(entry.path)
                  }}
                  onContextMenu={async (e) => {
                    e.preventDefault()
                    e.stopPropagation()
                    const items = [
                      { id: 'open', label: 'Open' },
                      { id: 'open-finder', label: 'Show in Finder' },
                      { id: 'copy-path', label: 'Copy Path' },
                    ]
                    const id = await showContextMenu(items)
                    if (id === 'open') {
                      useTabsStore.getState().openFileAsTab(entry.path)
                    } else if (id === 'open-finder') {
                      await daemonCliPost('fs/open-finder', { target: entry.path })
                    } else if (id === 'copy-path') {
                      await navigator.clipboard.writeText(entry.path).catch((err) => console.warn('[file-tree] clipboard write', err))
                    }
                  }}
                  title={entry.path}
                >
                  <svg className="w-3 h-3 text-yellow-500/80 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
                    <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
                    <path d="M7 11V7a5 5 0 0110 0v4" />
                  </svg>
                  <span className="truncate">{entry.path.startsWith(rootPath) ? entry.path.slice(rootPath.length + 1) : entry.name}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      )}

      {/* AI Config section */}
      {aiConfigEntries.length > 0 && (
        <div className="px-3 pt-2 pb-1 border-b border-[var(--color-border)]">
          <button
            className="w-full flex items-center gap-1.5 mb-1.5 text-left text-[10px] uppercase tracking-wider font-semibold text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors"
            onClick={() => setAiConfigCollapsed((prev) => !prev)}
          >
            <svg
              className={`w-2.5 h-2.5 flex-shrink-0 transition-transform ${aiConfigCollapsed ? '' : 'rotate-90'}`}
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2.5}
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
            </svg>
            <span className="flex-1">AI Config</span>
          </button>
          {!aiConfigCollapsed && (
            <div className="flex flex-wrap gap-1.5 pb-1.5">
              {aiConfigEntries.map((entry) => (
                <button
                  key={entry.path}
                  className="inline-flex items-center gap-1.5 px-2 py-1 text-[11px] bg-white/[0.04] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-white/[0.08] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors"
                  onClick={() => {
                    if (entry.isDirectory) {
                      // Expand directory in file tree and scroll to it
                      setExpandedDirs((prev) => new Set(prev).add(entry.path))
                      loadDir(entry.path)
                      useFileSelectionStore.getState().select(entry.path)
                    } else {
                      useFileSelectionStore.getState().select(entry.path)
                      useTabsStore.getState().openFileAsTab(entry.path)
                    }
                  }}
                  onContextMenu={async (e) => {
                    e.preventDefault()
                    e.stopPropagation()
                    const items = [
                      entry.isDirectory
                        ? { id: 'open', label: 'Reveal in Tree' }
                        : { id: 'open', label: 'Open' },
                      { id: 'open-finder', label: 'Show in Finder' },
                      { id: 'copy-path', label: 'Copy Path' },
                    ]
                    const id = await showContextMenu(items)
                    if (id === 'open') {
                      if (entry.isDirectory) {
                        setExpandedDirs((prev) => new Set(prev).add(entry.path))
                        loadDir(entry.path)
                        useFileSelectionStore.getState().select(entry.path)
                      } else {
                        useTabsStore.getState().openFileAsTab(entry.path)
                      }
                    } else if (id === 'open-finder') {
                      await daemonCliPost('fs/open-finder', { target: entry.path })
                    } else if (id === 'copy-path') {
                      await navigator.clipboard.writeText(entry.path).catch((err) => console.warn('[file-tree] clipboard write', err))
                    }
                  }}
                  title={entry.name}
                >
                  <svg className="w-3 h-3 text-purple-400/80 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
                    <path d="M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z" />
                    <path d="M20 3v4" />
                    <path d="M22 5h-4" />
                  </svg>
                  <span className="truncate">{entry.name}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Search input + hidden files toggle */}
      <div className="px-3 pt-2 pb-2 flex items-center gap-1.5">
        <div className="relative flex-1">
          <svg
            className="absolute left-2 top-1/2 -translate-y-1/2 w-3 h-3 text-[var(--color-text-muted)]"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth={2}
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Filter files..."
            className="w-full pl-7 pr-2 py-1.5 text-xs bg-white/[0.04] border border-[var(--color-border)]  text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] transition-colors"
          />
          {searchQuery && (
            <button
              className="absolute right-1.5 top-1/2 -translate-y-1/2 w-4 h-4 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
              onClick={() => setSearchQuery('')}
            >
              <svg
                className="w-3 h-3"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth={2}
              >
                <line x1="18" y1="6" x2="6" y2="18" />
                <line x1="6" y1="6" x2="18" y2="18" />
              </svg>
            </button>
          )}
        </div>
        {/* Hidden files toggle (eye icon) — open eye = showing hidden, crossed = hiding */}
        <button
          onClick={toggleHiddenFiles}
          title={`${showHiddenFiles ? 'Hide' : 'Show'} hidden files (⌘⇧.)`}
          className={`flex items-center justify-center w-7 h-7 border border-[var(--color-border)] transition-colors ${
            showHiddenFiles
              ? 'text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20'
              : 'text-[var(--color-text-muted)] bg-white/[0.04] hover:text-[var(--color-text-primary)] hover:bg-white/[0.08]'
          }`}
        >
          {showHiddenFiles ? (
            <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
              <circle cx="12" cy="12" r="3" />
            </svg>
          ) : (
            <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
              <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24" />
              <line x1="1" y1="1" x2="23" y2="23" />
            </svg>
          )}
        </button>
      </div>

      {/* Drop indicator banner */}
      {isDragOver && (
        <div className="mx-3 mb-2 px-2 py-1.5 text-[11px] text-[var(--color-accent)] border border-[var(--color-accent)] bg-[var(--color-accent)]/5 text-center">
          {optionKeyRef.current ? 'Copy here (Option held)' : 'Move here (hold Option to copy)'}
        </div>
      )}

      {/* Tree — root contents rendered directly (root is the implicit workspace) */}
      <div
        className="flex-1 overflow-y-auto overflow-x-hidden px-1 pb-2"
        data-path={rootPath}
        data-is-directory="true"
        onContextMenu={(e) => {
          // Right-click on empty space → context menu for root
          if ((e.target as HTMLElement).closest('[data-path]') === e.currentTarget) {
            handleContextMenu(e, {
              name: rootBasename,
              path: rootPath,
              isDirectory: true,
              size: 0,
              modifiedAt: 0
            })
          }
        }}
      >
        {loadingDirs.has(rootPath) && (
          <div className="py-1 pl-4 text-[11px] text-[var(--color-text-muted)] italic">
            Loading...
          </div>
        )}
        {errorDirs.has(rootPath) && (
          <div className="py-1 pl-4 text-[11px] text-red-400 italic">
            {errorDirs.get(rootPath)}
          </div>
        )}
        {newEntryState && newEntryState.parentPath === rootPath && (
          <InlineNameEditor
            initialValue=""
            depth={0}
            icon={newEntryState.isDirectory ? <FolderIcon open={false} /> : <FileIcon />}
            onConfirm={(name) => handleNewEntryConfirm(rootPath, name, newEntryState.isDirectory)}
            onCancel={handleNewEntryCancel}
          />
        )}
        {filteredRootEntries?.map((entry) => (
          <TreeItem key={entry.path} entry={entry} depth={0} {...childProps} />
        ))}
        {filteredRootEntries && filteredRootEntries.length === 0 && !newEntryState && !loadingDirs.has(rootPath) && (
          <div className="py-1 pl-4 text-[11px] text-[var(--color-text-muted)] italic">
            {searchQuery ? 'No matches' : 'Empty'}
          </div>
        )}
      </div>
    </div>
  )
}
