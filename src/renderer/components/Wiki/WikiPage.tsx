// Workspace Knowledge Base / brain map — full-page overlay
// (prd-workspace-kb-brain-map-and-publish §7.2).
//
// Pattern mirrors FeedbackPage / ProjectsPage: fixed-inset overlay with
// its own top bar (ServerSwitcher + PageTabs). Opened
// only from WorkspacePanel → View Wiki; not a permanent PageTabs entry.
// Esc (or close) returns to Agents.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import remarkGfm from 'remark-gfm'
import { titleBarDragOnMouseDown, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { useToastStore } from '@/stores/toast'
import ServerSwitcher from '@/components/TopBar/ServerSwitcher'
import PageTabs from '@/components/TopBar/PageTabs'
import DesktopChromeLeft from '@/components/TopBar/DesktopChromeLeft'
import DesktopChromeRight from '@/components/TopBar/DesktopChromeRight'
import TimerButton from '@/components/Timer/TimerButton'
import K2NounsCheatSheet from '@/components/CheatSheet/K2NounsCheatSheet'
import ModeToggle from '@/components/Presence/ModeToggle'
import { Surface } from '@/components/ui'
import Markdown from '@/components/Markdown/Markdown'
import WikiGraph from './WikiGraph'
import { FocusGroupFilterDropdown } from './FocusGroupFilterDropdown'
import {
  WorkspaceFilterDropdown,
  type FilterableWorkspace,
} from '@/components/Feedback/WorkspaceFilterDropdown'
import {
  fetchWikiIndex,
  fetchWikiNote,
  fetchWikiServeStatus,
  preprocessWikilinks,
  resolveWikiTarget,
  absoluteWikiNotePath,
  seedWiki,
  setWikiServe,
  setWikiPublicChat,
  wikiIndexFingerprint,
  countVisibleWikiArticles,
  findWikiHomeNode,
  type WikiIndex,
  type WikiNote,
  type WikiServeStatus,
} from './wiki-api'

const TOPBAR_HEIGHT = 38
/** Quiet poll for new notes — only applies state when structure changes. */
const POLL_MS = 3_000

export default function WikiPage(): React.JSX.Element | null {
  const page = usePageViewStore((s) => s.page)
  const wikiProjectPath = usePageViewStore((s) => s.wikiProjectPath)
  const closeWiki = usePageViewStore((s) => s.closeWiki)
  const isOpen = page === 'wiki'
  const projects = useProjectsStore((s) => s.projects)

  const projectPath = wikiProjectPath
  const workspace = useMemo(
    () => (projectPath ? projects.find((p) => p.path === projectPath) ?? null : null),
    [projects, projectPath],
  )

  const [index, setIndex] = useState<WikiIndex | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [note, setNote] = useState<WikiNote | null>(null)
  const [noteError, setNoteError] = useState<string | null>(null)
  const [noteLoading, setNoteLoading] = useState(false)
  const [search, setSearch] = useState('')
  /** k2 = fleet map (all workspace brains); global/local = this workspace only. */
  const [mode, setMode] = useState<'k2' | 'local' | 'global'>('global')
  /** K2 sub-tab: Projects (V1) vs Focus Groups map. */
  const [k2Lens, setK2Lens] = useState<'projects' | 'groups'>('projects')
  const [depth, setDepth] = useState<1 | 2>(2)
  /** K2 Groups lens: `all` | `ungrouped` | focus group id. */
  const [focusGroupFilter, setFocusGroupFilter] = useState<string>('all')
  /** K2 Projects lens: same values as Feedback WorkspaceFilterDropdown. */
  const [projectFilter, setProjectFilter] = useState<string>('all')
  const [serve, setServe] = useState<WikiServeStatus | null>(null)
  const [serveBusy, setServeBusy] = useState(false)
  const [chatBusy, setChatBusy] = useState(false)
  const [seedBusy, setSeedBusy] = useState(false)
  /** Collapse the right-hand note reader so the graph can use full width. */
  const [readerCollapsed, setReaderCollapsed] = useState(false)
  /** Last applied graph structure — skip setState when poll finds no change. */
  const indexFpRef = React.useRef<string>('')
  /**
   * When true, next matching index should select Home (enter Global, or
   * empty selection after a mode/workspace reload).
   */
  const preferHomeRef = React.useRef(true)

  const loadIndex = useCallback(async (quiet = false): Promise<void> => {
    // Fleet (k2) map does not require a project; workspace maps do.
    if (mode !== 'k2' && !projectPath) return
    if (!quiet) setLoading(true)
    try {
      const data =
        mode === 'k2'
          ? await fetchWikiIndex(projectPath ?? '', { scope: 'k2' })
          : await fetchWikiIndex(projectPath!)
      const fp = wikiIndexFingerprint(data)
      // Only push a new index object when nodes/links/titles actually change.
      // Full replace every poll restarts force-graph and "re-stabilizes".
      if (fp !== indexFpRef.current) {
        indexFpRef.current = fp
        setIndex(data)
      }
      setLoadError(null)
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      // Empty / missing wiki is an empty state, not a hard failure.
      if (/not found|no wiki|does not exist|404/i.test(msg)) {
        const empty: WikiIndex = {
          workspacePath: projectPath ?? '',
          generatedAt: new Date().toISOString(),
          nodes: [],
          links: [],
          noteCount: 0,
          scope: mode === 'k2' ? 'k2' : 'workspace',
        }
        const fp = wikiIndexFingerprint(empty)
        if (fp !== indexFpRef.current) {
          indexFpRef.current = fp
          setIndex(empty)
        }
        setLoadError(null)
      } else {
        setLoadError(msg)
      }
    } finally {
      if (!quiet) setLoading(false)
    }
  }, [projectPath, mode])

  const loadServe = useCallback(async (): Promise<void> => {
    if (!projectPath) return
    try {
      const st = await fetchWikiServeStatus(projectPath)
      setServe(st)
    } catch {
      setServe({ enabled: false })
    }
  }, [projectPath])

  // Fetch index + serve status while open. Mode changes reload (workspace vs fleet).
  useEffect(() => {
    if (!isOpen) return
    if (mode !== 'k2' && !projectPath) return
    // Clear fingerprint so mode switch always applies the new graph.
    indexFpRef.current = ''
    // Global always resurfaces Home; other modes only auto-pick when empty.
    preferHomeRef.current = mode === 'global' || mode === 'k2'
    setSelectedId(null)
    setNote(null)
    void loadIndex()
    if (projectPath) void loadServe()
    const id = window.setInterval(() => {
      void loadIndex(true)
    }, POLL_MS)
    return () => window.clearInterval(id)
  }, [isOpen, projectPath, mode, loadIndex, loadServe])

  // Load selected note body — only when selection changes (not on index poll).
  useEffect(() => {
    if (!isOpen || !selectedId) {
      return
    }
    const node = index?.nodes.find((n) => n.id === selectedId)
    if (node && !node.exists) {
      setNote(null)
      setNoteError('This note does not exist yet.')
      setNoteLoading(false)
      return
    }
    const noteProject =
      node?.workspacePath || note?.workspacePath || projectPath
    let cancelled = false
    setNoteLoading(true)
    setNoteError(null)
    fetchWikiNote(noteProject, selectedId)
      .then((n) => {
        if (cancelled) return
        setNote(n)
        setNoteError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setNote(null)
        setNoteError(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        if (!cancelled) setNoteLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [isOpen, projectPath, selectedId]) // eslint-disable-line react-hooks/exhaustive-deps

  // Soft-refresh open note body when its file appears/changes (no full-page thrash).
  useEffect(() => {
    if (!isOpen || !selectedId || !index) return
    const node = index.nodes.find((n) => n.id === selectedId)
    if (!node?.exists) return
    const noteProject = node.workspacePath || projectPath
    let cancelled = false
    fetchWikiNote(noteProject, selectedId)
      .then((n) => {
        if (cancelled) return
        setNote((prev) => {
          if (prev && prev.id === n.id && prev.body === n.body && prev.title === n.title) {
            return prev
          }
          return n
        })
      })
      .catch(() => {
        /* keep showing previous body */
      })
    return () => {
      cancelled = true
    }
  }, [isOpen, projectPath, selectedId, index])

  // Auto-select Home (Global always; otherwise when nothing / stale selection).
  useEffect(() => {
    if (!isOpen || !index || index.nodes.length === 0) return

    // Wait for index that matches mode — avoid pinning fleet Home while
    // switching K2 → Global before the workspace index arrives.
    const indexIsFleet = index.scope === 'k2'
    if (mode === 'k2' && !indexIsFleet) return
    if (mode !== 'k2' && indexIsFleet) return

    const home = findWikiHomeNode(index.nodes)
    if (!home) return

    const selectionMissing =
      selectedId != null && !index.nodes.some((n) => n.id === selectedId)

    if (preferHomeRef.current || selectedId == null || selectionMissing) {
      preferHomeRef.current = false
      if (selectedId !== home.id) setSelectedId(home.id)
    }
  }, [isOpen, index, selectedId, mode])

  // Esc → close wiki (or clear selection first).
  useEffect(() => {
    if (!isOpen) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      if (selectedId !== null) setSelectedId(null)
      else closeWiki()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [isOpen, selectedId, closeWiki])

  // Reset transient state when closed.
  useEffect(() => {
    if (!isOpen) {
      setIndex(null)
      indexFpRef.current = ''
      setLoadError(null)
      setSelectedId(null)
      setNote(null)
      setNoteError(null)
      setSearch('')
      setMode('global')
      setK2Lens('projects')
      setDepth(2)
      setFocusGroupFilter('all')
      setProjectFilter('all')
      setServe(null)
      setReaderCollapsed(false)
      preferHomeRef.current = true
    }
  }, [isOpen])

  const selectByTarget = useCallback(
    (target: string): void => {
      if (!index) return
      const resolved = resolveWikiTarget(index, target)
      if (resolved) {
        setSelectedId(resolved.id)
        return
      }
      // Unresolved — select or invent a missing node id if present in links.
      const missing = index.nodes.find(
        (n) => !n.exists && (n.title === target || n.id === target),
      )
      if (missing) {
        setSelectedId(missing.id)
        return
      }
      useToastStore.getState().addToast(`Note not found: ${target}`, 'error')
    },
    [index],
  )

  const onSeed = async (): Promise<void> => {
    if (!projectPath || seedBusy) return
    setSeedBusy(true)
    try {
      await seedWiki(projectPath)
      await loadIndex()
      useToastStore.getState().addToast('Wiki seeded (Home.md + _Index.md)', 'success')
    } catch (e) {
      useToastStore
        .getState()
        .addToast(`Seed failed: ${e instanceof Error ? e.message : String(e)}`, 'error')
    } finally {
      setSeedBusy(false)
    }
  }

  const onToggleServe = async (): Promise<void> => {
    if (!projectPath || serveBusy) return
    const next = !(serve?.enabled)
    setServeBusy(true)
    try {
      const st = await setWikiServe(projectPath, next)
      setServe(st)
      if (next && st.url) {
        useToastStore.getState().addToast(`Serving wiki at ${st.url}`, 'success')
      } else if (!next) {
        useToastStore.getState().addToast('Wiki local server stopped', 'success')
      }
    } catch (e) {
      useToastStore
        .getState()
        .addToast(`Serve toggle failed: ${e instanceof Error ? e.message : String(e)}`, 'error')
    } finally {
      setServeBusy(false)
    }
  }

  const onTogglePublicChat = async (): Promise<void> => {
    if (!projectPath || chatBusy) return
    const next = !(serve?.publicChatEnabled)
    setChatBusy(true)
    try {
      const st = await setWikiPublicChat(projectPath, next)
      setServe(st)
      if (next) {
        if (st.publicChatReady) {
          useToastStore
            .getState()
            .addToast(
              'Public chat on — visitors with the wiki URL can ask this workspace agent',
              'success',
            )
        } else {
          useToastStore
            .getState()
            .addToast(
              st.publicChatError
                ? `Public chat enabled but not ready: ${st.publicChatError}`
                : 'Public chat enabled but not ready — check API is on',
              'error',
            )
        }
      } else {
        useToastStore.getState().addToast('Public chat off', 'success')
      }
    } catch (e) {
      useToastStore
        .getState()
        .addToast(
          `Public chat toggle failed: ${e instanceof Error ? e.message : String(e)}`,
          'error',
        )
    } finally {
      setChatBusy(false)
    }
  }

  const onOpenInEditor = (): void => {
    if (!note) {
      useToastStore.getState().addToast('No file path for this note', 'error')
      return
    }
    const path = absoluteWikiNotePath(note, projectPath)
    if (!path) {
      useToastStore.getState().addToast('No file path for this note', 'error')
      return
    }
    closeWiki()
    // Next tick so overlay unmounts before tab focus shifts.
    window.setTimeout(() => {
      useTabsStore.getState().openFileAsTab(path)
    }, 0)
  }

  const renderedBody = useMemo(() => {
    if (!note?.body) return ''
    return preprocessWikilinks(note.body)
  }, [note?.body])

  const selectedNode = useMemo(
    () => (selectedId && index ? index.nodes.find((n) => n.id === selectedId) ?? null : null),
    [index, selectedId],
  )

  /** Live article count for toolbar tag — follows search + K2/Global/Local + filters. */
  const articleCount = useMemo(() => {
    if (!index) return null
    return countVisibleWikiArticles(index, {
      search,
      mode,
      depth,
      selectedId,
      focusGroupFilter,
      k2Lens,
      projectFilter,
    })
  }, [index, search, mode, depth, selectedId, focusGroupFilter, k2Lens, projectFilter])

  const fleetGroups = index?.groups ?? []
  const showFocusGroupFilter = mode === 'k2' && k2Lens === 'groups' && fleetGroups.length > 0
  const showProjectFilter = mode === 'k2' && k2Lens === 'projects'

  /** Same shape Feedback's WorkspaceFilterDropdown expects. */
  const filterableWorkspaces = useMemo((): FilterableWorkspace[] => {
    return projects.map((p) => ({
      id: p.id,
      name: p.name,
      path: p.path,
      color: p.color,
      iconUrl: p.iconUrl ?? null,
      focusGroupId: p.focusGroupId ?? null,
    }))
  }, [projects])

  const isEmpty = index !== null && index.noteCount === 0 && index.nodes.filter((n) => n.exists).length === 0

  if (!isOpen) return null

  return (
    <div className="fixed inset-[var(--inset-window)] z-50 flex flex-col bg-[var(--color-bg)]">
      <Surface
        role2="surface"
        bordered={false}
        className="flex items-center border-b border-[var(--color-border)] px-3 select-none flex-shrink-0"
        onMouseDown={titleBarDragOnMouseDown}
        onDoubleClick={titleBarOnDoubleClick}
        style={{ height: TOPBAR_HEIGHT, minHeight: TOPBAR_HEIGHT }}
      >
        <div className="flex items-center gap-2 flex-1">
          <DesktopChromeLeft />
          <span className="text-[10px] font-bold tracking-widest text-[var(--color-text-muted)] uppercase flex-shrink-0">
            K2
          </span>
          <ServerSwitcher />
          <div className="no-drag">
            <PageTabs />
          </div>
          <span className="no-drag text-[11px] text-[var(--color-text-muted)] truncate ml-2">
            Wiki
            {workspace ? (
              <span className="text-[var(--color-text-secondary)]"> · {workspace.name}</span>
            ) : projectPath ? (
              <span className="text-[var(--color-text-secondary)]"> · {projectPath.split('/').pop()}</span>
            ) : null}
          </span>
        </div>

        <DesktopChromeRight>
          <div className="flex items-center gap-2 no-drag">
            <TimerButton />
            <K2NounsCheatSheet />
            <ModeToggle />
            <div className="w-px h-4 bg-[var(--color-border)]" />
            <button
              type="button"
              onClick={closeWiki}
              className="flex items-center justify-center w-7 h-7 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] transition-colors cursor-pointer"
              title="Close (Esc)"
            >
              <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
                <line x1="2" y1="2" x2="10" y2="10" />
                <line x1="10" y1="2" x2="2" y2="10" />
              </svg>
            </button>
          </div>
        </DesktopChromeRight>
      </Surface>

      {/* Toolbar */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex-wrap">
        <div className="relative w-96 flex-shrink-0">
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search notes, tags, aliases…"
            className="w-full px-2.5 py-1.5 pr-7 text-[11px] bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)]"
          />
          {search && (
            <button
              type="button"
              onClick={() => setSearch('')}
              aria-label="Clear search"
              className="absolute right-1.5 top-1/2 -translate-y-1/2 flex items-center justify-center w-4 h-4 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
            >
              <svg width="9" height="9" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
                <line x1="2" y1="2" x2="10" y2="10" />
                <line x1="10" y1="2" x2="2" y2="10" />
              </svg>
            </button>
          )}
        </div>

        {articleCount !== null && (
          <div
            className="flex items-center gap-1.5 text-[10px] text-[var(--color-text-muted)] select-none"
            title="Notes matching the current scope and search"
          >
            <span>Articles</span>
            <span className="inline-flex items-center justify-center min-w-[1.25rem] px-1.5 py-0.5 text-[10px] font-medium tabular-nums border border-[var(--color-border)] bg-white/[0.06] text-[var(--color-text-secondary)]">
              {articleCount}
            </span>
          </div>
        )}

        <div className="flex items-center gap-0.5 border border-[var(--color-border)]">
          <button
            type="button"
            onClick={() => setMode('k2')}
            className={`px-2 py-1 text-[10px] font-medium transition-colors cursor-pointer ${
              mode === 'k2'
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
            }`}
            title="All workspace brains (host ~/.k2/wiki registry)"
          >
            K2
          </button>
          <button
            type="button"
            onClick={() => setMode('global')}
            className={`px-2 py-1 text-[10px] font-medium transition-colors cursor-pointer ${
              mode === 'global'
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
            }`}
            title="Full knowledge graph for this workspace"
          >
            Global
          </button>
          <button
            type="button"
            onClick={() => setMode('local')}
            className={`px-2 py-1 text-[10px] font-medium transition-colors cursor-pointer ${
              mode === 'local'
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
            }`}
            title="Neighborhood of the selected note"
          >
            Local
          </button>
        </div>

        {mode === 'k2' && (
          <div className="flex items-center gap-0.5 border border-[var(--color-border)] flex-shrink-0">
            {([
              { id: 'projects' as const, label: 'Projects', title: 'Project hubs → workspace brains' },
              { id: 'groups' as const, label: 'Groups', title: 'Focus group hubs → workspace brains' },
            ]).map((tab) => (
              <button
                key={tab.id}
                type="button"
                onClick={() => setK2Lens(tab.id)}
                title={tab.title}
                className={`px-2 py-1 text-[10px] font-medium transition-colors cursor-pointer ${
                  k2Lens === tab.id
                    ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                    : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
                }`}
              >
                {tab.label}
              </button>
            ))}
          </div>
        )}

        {mode === 'local' && (
          <div className="flex items-center gap-1 text-[10px] text-[var(--color-text-muted)]">
            <span>Depth</span>
            {([1, 2] as const).map((d) => (
              <button
                key={d}
                type="button"
                onClick={() => setDepth(d)}
                className={`w-6 h-6 border transition-colors cursor-pointer ${
                  depth === d
                    ? 'border-[var(--color-accent)] text-[var(--color-text-primary)] bg-[var(--color-accent)]/15'
                    : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-muted)]'
                }`}
              >
                {d}
              </button>
            ))}
          </div>
        )}

        {showProjectFilter && (
          <div className="flex items-center gap-1.5 text-[10px] text-[var(--color-text-muted)] flex-shrink-0">
            <span className="flex-shrink-0">Filter</span>
            <WorkspaceFilterDropdown
              projects={filterableWorkspaces}
              value={projectFilter}
              onChange={setProjectFilter}
            />
          </div>
        )}

        {showFocusGroupFilter && (
          <div className="flex items-center gap-1.5 text-[10px] text-[var(--color-text-muted)] flex-shrink-0">
            <span className="flex-shrink-0">Focus Group</span>
            <FocusGroupFilterDropdown
              groups={fleetGroups}
              value={focusGroupFilter}
              onChange={setFocusGroupFilter}
            />
          </div>
        )}

        <div className="flex-1" />

        <button
          type="button"
          disabled={seedBusy || !projectPath}
          onClick={() => void onSeed()}
          className="px-2 py-1 text-[10px] font-medium border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors cursor-pointer disabled:opacity-50"
          title="Create Home.md and _Index.md under .k2/wiki/"
        >
          {seedBusy ? 'Seeding…' : 'Seed wiki'}
        </button>

        <button
          type="button"
          disabled={serveBusy || !projectPath}
          onClick={() => void onToggleServe()}
          className={`px-2 py-1 text-[10px] font-medium border transition-colors cursor-pointer disabled:opacity-50 ${
            serve?.enabled
              ? 'border-[var(--color-status-ok-soft)] text-[var(--color-status-ok-soft)] bg-[color-mix(in_srgb,var(--color-status-ok-soft)_10%,transparent)]'
              : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
          }`}
          title={serve?.enabled ? `Stop local server (${serve.url ?? 'on'})` : 'Serve read-only site on localhost'}
        >
          {serveBusy ? '…' : serve?.enabled ? 'Serve: ON' : 'Serve: OFF'}
        </button>

        <button
          type="button"
          disabled={chatBusy || !projectPath}
          onClick={() => void onTogglePublicChat()}
          className={`px-2 py-1 text-[10px] font-medium border transition-colors cursor-pointer disabled:opacity-50 ${
            serve?.publicChatEnabled
              ? serve.publicChatReady
                ? 'border-[var(--color-status-ok-soft)] text-[var(--color-status-ok-soft)] bg-[color-mix(in_srgb,var(--color-status-ok-soft)_10%,transparent)]'
                : 'border-[var(--color-status-warn-soft,var(--color-border))] text-[var(--color-status-warn-soft,var(--color-text-secondary))] bg-[color-mix(in_srgb,var(--color-status-warn-soft,var(--color-border))_10%,transparent)]'
              : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
          }`}
          title={
            serve?.publicChatEnabled
              ? serve.publicChatReady
                ? 'Public chat on — visitors with the wiki URL can ask this workspace agent. Click to disable.'
                : `Public chat enabled but not ready: ${serve.publicChatError ?? 'check API is on'}`
              : 'Allow public chat: visitors with the served/published wiki URL can ask this workspace agent (default off)'
          }
        >
          {chatBusy
            ? '…'
            : serve?.publicChatEnabled
              ? serve.publicChatReady
                ? 'Public chat: ON'
                : 'Public chat: …'
              : 'Public chat: OFF'}
        </button>

        {serve?.enabled && serve.url && (
          <a
            href={serve.url}
            target="_blank"
            rel="noreferrer"
            className="text-[10px] text-[var(--color-accent)] hover:underline truncate max-w-[180px]"
            title={serve.url}
          >
            {serve.url}
          </a>
        )}

        <button
          type="button"
          onClick={() => setReaderCollapsed((c) => !c)}
          className={`px-2 py-1 text-[10px] font-medium border transition-colors cursor-pointer flex items-center gap-1 ${
            readerCollapsed
              ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
              : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)]'
          }`}
          title={readerCollapsed ? 'Show article viewer' : 'Collapse article viewer'}
        >
          <svg
            width="10"
            height="10"
            viewBox="0 0 12 12"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            className={`flex-shrink-0 transition-transform ${readerCollapsed ? '' : 'rotate-180'}`}
          >
            <path d="M4 2 L8 6 L4 10" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
          {readerCollapsed ? 'Show article' : 'Hide article'}
        </button>
      </div>

      {/* Body: graph | reader */}
      <div
        className={`flex-1 min-h-0 grid ${
          readerCollapsed
            ? 'grid-cols-1 grid-rows-[minmax(0,1fr)]'
            : 'grid-cols-1 grid-rows-[minmax(0,1fr)_minmax(0,1fr)] lg:grid-cols-[minmax(0,1.2fr)_minmax(280px,0.9fr)] lg:grid-rows-[minmax(0,1fr)]'
        }`}
      >
        <div
          className={`relative min-h-0 min-w-0 bg-[var(--color-bg)] ${
            readerCollapsed
              ? ''
              : 'border-b lg:border-b-0 lg:border-r border-[var(--color-border)]'
          }`}
        >
          {!projectPath && (
            <div className="absolute inset-0 flex items-center justify-center">
              <p className="text-sm text-[var(--color-text-muted)]">No workspace selected.</p>
            </div>
          )}
          {projectPath && loading && !index && (
            <div className="absolute inset-0 flex items-center justify-center">
              <p className="text-sm text-[var(--color-text-muted)]">Loading knowledge base…</p>
            </div>
          )}
          {loadError && (
            <div className="absolute inset-0 flex items-center justify-center px-6">
              <p className="text-[11px] text-[var(--color-status-error-soft)] text-center">
                Failed to load wiki: {loadError}
              </p>
            </div>
          )}
          {index && isEmpty && !loadError && (
            <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 px-6">
              <p className="text-sm text-[var(--color-text-primary)]">No knowledge base yet</p>
              <p className="text-[11px] text-[var(--color-text-muted)] text-center max-w-sm">
                Notes live in <code className="text-[var(--color-text-secondary)]">.k2/wiki/</code> as
                Markdown with <code className="text-[var(--color-text-secondary)]">[[wikilinks]]</code>.
                Seed a Home + Index, or drop notes there.
              </p>
              <button
                type="button"
                disabled={seedBusy}
                onClick={() => void onSeed()}
                className="px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 transition-opacity cursor-pointer disabled:opacity-50"
              >
                {seedBusy ? 'Seeding…' : 'Seed wiki'}
              </button>
            </div>
          )}
          {index && !isEmpty && !loadError && (
            <WikiGraph
              index={index}
              selectedId={selectedId}
              search={search}
              mode={mode}
              depth={depth}
              k2Lens={k2Lens}
              focusGroupFilter={focusGroupFilter}
              projectFilter={projectFilter}
              onSelect={(id) => {
                setSelectedId(id)
                if (id === null) {
                  setNote(null)
                  setNoteError(null)
                }
              }}
            />
          )}
        </div>

        {/* Reader */}
        {!readerCollapsed && (
        <div className="flex flex-col min-h-0 min-w-0 bg-[var(--color-bg-surface)]">
          {!selectedId && (
            <div className="flex-1 flex items-center justify-center px-6">
              <p className="text-[11px] text-[var(--color-text-muted)] text-center border border-dashed border-[var(--color-border)] px-6 py-8 w-full max-w-sm">
                Select a note on the graph to read it here.
              </p>
            </div>
          )}
          {selectedId && (
            <>
              <div className="flex items-start gap-2 px-4 py-3 border-b border-[var(--color-border)] flex-shrink-0">
                <div className="flex-1 min-w-0">
                  <h2 className="text-sm font-medium text-[var(--color-text-primary)] truncate">
                    {note?.title ?? selectedNode?.title ?? selectedId}
                  </h2>
                  {(selectedNode?.workspaceName || note?.workspacePath) && (
                    <p className="text-[10px] text-[var(--color-text-muted)] truncate mt-0.5">
                      {selectedNode?.workspaceName ?? 'Workspace'}
                    </p>
                  )}
                  {((note?.tags?.length ?? 0) > 0 || (selectedNode?.tags?.length ?? 0) > 0) && (
                    <div className="flex flex-wrap gap-1 mt-1.5">
                      {(note?.tags ?? selectedNode?.tags ?? []).map((t) => (
                        <span
                          key={t}
                          className="px-1.5 py-0.5 text-[9px] uppercase tracking-wide bg-white/[0.06] text-[var(--color-text-muted)]"
                        >
                          {t}
                        </span>
                      ))}
                    </div>
                  )}
                </div>
                <button
                  type="button"
                  onClick={onOpenInEditor}
                  disabled={!note || !absoluteWikiNotePath(note, projectPath)}
                  className="flex-shrink-0 px-2 py-1 text-[10px] font-medium border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors cursor-pointer disabled:opacity-40"
                  title="Open note file in the editor"
                >
                  Open in editor
                </button>
              </div>
              <div className="flex-1 overflow-auto p-4 min-h-0">
                {noteLoading && (
                  <p className="text-[11px] text-[var(--color-text-muted)]">Loading…</p>
                )}
                {!noteLoading && noteError && (
                  <p className="text-[11px] text-[var(--color-text-muted)] italic">{noteError}</p>
                )}
                {!noteLoading && !noteError && note && (
                  <div className="markdown-content">
                    <Markdown
                      remarkPlugins={[remarkGfm]}
                      components={{
                        a: ({ href, children, ...props }) => {
                          if (href?.startsWith('wiki://')) {
                            const target = decodeURIComponent(href.slice('wiki://'.length))
                            return (
                              <a
                                href={href}
                                className="text-[var(--color-accent)] underline decoration-[var(--color-accent)]/40 hover:decoration-[var(--color-accent)] cursor-pointer"
                                onClick={(e) => {
                                  e.preventDefault()
                                  selectByTarget(target)
                                }}
                              >
                                {children}
                              </a>
                            )
                          }
                          return (
                            <a href={href} {...props}>
                              {children}
                            </a>
                          )
                        },
                      }}
                    >
                      {renderedBody || '*Empty note*'}
                    </Markdown>
                  </div>
                )}
              </div>
            </>
          )}
        </div>
        )}
      </div>
    </div>
  )
}
