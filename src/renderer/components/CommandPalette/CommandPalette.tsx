import { useEffect, useRef, useState, useCallback, useMemo } from 'react'
import { useCommandPaletteStore } from '../../stores/command-palette'
import { useProjectsStore } from '../../stores/projects'
import { useFocusGroupsStore } from '../../stores/focus-groups'
import { useSidebarStore } from '../../stores/sidebar'
import ProjectAvatar from '../Sidebar/ProjectAvatar'

interface FocusGroupResult {
  type: 'focus-group'
  id: string
  name: string
  color: string | null
  projectCount: number
}

interface ProjectResult {
  type: 'project'
  id: string
  name: string
  path: string
  color: string
  focusGroupId: string | null
  iconUrl?: string | null
}

type Result = FocusGroupResult | ProjectResult

export default function CommandPalette(): React.JSX.Element | null {
  const isOpen = useCommandPaletteStore((s) => s.isOpen)
  const close = useCommandPaletteStore((s) => s.close)

  const projects = useProjectsStore((s) => s.projects)
  const setActiveProject = useProjectsStore((s) => s.setActiveProject)
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)

  const focusGroups = useFocusGroupsStore((s) => s.focusGroups)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const setActiveFocusGroup = useFocusGroupsStore((s) => s.setActiveFocusGroup)

  const expand = useSidebarStore((s) => s.expand)

  const [query, setQuery] = useState('')
  const [selectedIndex, setSelectedIndex] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)

  // Reset state when opening
  useEffect(() => {
    if (isOpen) {
      setQuery('')
      setSelectedIndex(0)
      // Focus input after mount
      requestAnimationFrame(() => {
        inputRef.current?.focus()
      })
    }
  }, [isOpen])

  // Fuzzy match: each space-separated token must appear somewhere in the target.
  // "k2 web" matches "k2so-website", "my proj" matches "my-cool-project".
  // Also supports sequential character matching as fallback:
  // "kweb" matches "k2so-website" (k...w...e...b in order).
  const fuzzyMatch = useCallback((target: string, query: string): boolean => {
    if (!query) return true
    const t = target.toLowerCase()
    const tokens = query.toLowerCase().split(/\s+/).filter(Boolean)

    // Token match: every token must appear as a substring
    const tokenMatch = tokens.every((tok) => t.includes(tok))
    if (tokenMatch) return true

    // Sequential character match: each char of query appears in order in target
    const chars = query.toLowerCase().replace(/\s+/g, '')
    let ti = 0
    for (let ci = 0; ci < chars.length; ci++) {
      const idx = t.indexOf(chars[ci], ti)
      if (idx === -1) return false
      ti = idx + 1
    }
    return true
  }, [])

  // Build results list
  const results = useMemo((): Result[] => {
    const q = query.trim()
    const items: Result[] = []

    // Focus groups (only if enabled)
    if (focusGroupsEnabled) {
      const matchingGroups = focusGroups.filter(
        (g) => fuzzyMatch(g.name, q)
      )
      for (const g of matchingGroups) {
        const count = projects.filter((p) => p.focusGroupId === g.id).length
        items.push({
          type: 'focus-group',
          id: g.id,
          name: g.name,
          color: g.color,
          projectCount: count
        })
      }
    }

    // Projects
    const matchingProjects = projects.filter(
      (p) => fuzzyMatch(p.name, q) || fuzzyMatch(p.path, q)
    )
    for (const p of matchingProjects) {
      items.push({
        type: 'project',
        id: p.id,
        name: p.name,
        path: p.path,
        color: p.color,
        focusGroupId: p.focusGroupId,
        iconUrl: p.iconUrl
      })
    }

    return items
  }, [query, focusGroups, projects, focusGroupsEnabled])

  // Clamp selected index when results change
  useEffect(() => {
    setSelectedIndex((prev) => Math.min(prev, Math.max(0, results.length - 1)))
  }, [results.length])

  // Select a result
  const selectResult = useCallback(
    (result: Result) => {
      if (result.type === 'focus-group') {
        // autoActivate: false — we pick + activate the first project
        // ourselves below; the store's nested activation would switch
        // to it TWICE (stash/restore churn + pinned-tab race).
        setActiveFocusGroup(result.id, { autoActivate: false })
        // Find first project in this group
        const firstProject = projects.find((p) => p.focusGroupId === result.id)
        if (firstProject) {
          setActiveProject(firstProject.id)
          if (firstProject.workspaces[0]) {
            setActiveWorkspace(firstProject.id, firstProject.workspaces[0].id)
          }
        }
      } else {
        // Project
        if (result.focusGroupId && focusGroupsEnabled) {
          // autoActivate: false — the selected project is activated
          // explicitly below (double-switch race otherwise).
          setActiveFocusGroup(result.focusGroupId, { autoActivate: false })
        }
        setActiveProject(result.id)
        const project = projects.find((p) => p.id === result.id)
        if (project?.workspaces[0]) {
          setActiveWorkspace(project.id, project.workspaces[0].id)
        }
      }
      expand()
      close()
    },
    [projects, focusGroupsEnabled, setActiveFocusGroup, setActiveProject, setActiveWorkspace, expand, close]
  )

  // Keyboard handler
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        close()
        return
      }

      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setSelectedIndex((prev) => Math.min(prev + 1, results.length - 1))
        return
      }

      if (e.key === 'ArrowUp') {
        e.preventDefault()
        setSelectedIndex((prev) => Math.max(prev - 1, 0))
        return
      }

      if (e.key === 'Enter') {
        e.preventDefault()
        if (results[selectedIndex]) {
          selectResult(results[selectedIndex])
        }
        return
      }
    },
    [close, results, selectedIndex, selectResult]
  )

  // Scroll selected item into view
  useEffect(() => {
    const list = listRef.current
    if (!list) return
    const item = list.children[selectedIndex] as HTMLElement | undefined
    if (item) {
      item.scrollIntoView({ block: 'nearest' })
    }
  }, [selectedIndex])

  if (!isOpen) return null

  // Split results into sections for rendering
  const focusGroupResults = results.filter((r): r is FocusGroupResult => r.type === 'focus-group')
  const projectResults = results.filter((r): r is ProjectResult => r.type === 'project')

  // Calculate global index offset for project section
  const projectIndexOffset = focusGroupResults.length

  return (
    <div
      className="fixed inset-0 z-[9999] flex items-start justify-center pt-[15vh] no-drag"
      style={{ background: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(8px)' }}
      onClick={(e) => {
        if (e.target === e.currentTarget) close()
      }}
      onKeyDown={handleKeyDown}
    >
      <div
        className="w-[560px] max-h-[60vh] flex flex-col overflow-hidden border border-[var(--color-border)]"
        style={{ background: 'var(--color-bg-surface)', boxShadow: '0 24px 48px rgba(0, 0, 0, 0.5)' }}
      >
        {/* Search input */}
        <div className="flex items-center border-b border-[var(--color-border)] px-4 py-3">
          <svg
            width="14"
            height="14"
            viewBox="0 0 16 16"
            fill="none"
            className="flex-shrink-0 mr-3"
            style={{ color: 'var(--color-text-muted)' }}
          >
            <path
              d="M11.5 7a4.5 4.5 0 1 1-9 0 4.5 4.5 0 0 1 9 0ZM10.643 11.357a6 6 0 1 1 .714-.714l3.85 3.85a.5.5 0 0 1-.707.707l-3.857-3.843Z"
              fill="currentColor"
            />
          </svg>
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value)
              setSelectedIndex(0)
            }}
            placeholder="Search workspaces and focus groups..."
            spellCheck={false}
            autoComplete="off"
            className="flex-1 bg-transparent text-sm text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none"
            style={{ fontFamily: 'inherit' }}
          />
          <kbd
            className="ml-2 text-[10px] px-1.5 py-0.5 border border-[var(--color-border)] text-[var(--color-text-muted)]"
            style={{ background: 'var(--color-bg)' }}
          >
            ESC
          </kbd>
        </div>

        {/* Results */}
        <div ref={listRef} className="overflow-y-auto flex-1" style={{ scrollbarWidth: 'thin' }}>
          {results.length === 0 && (
            <div className="px-4 py-8 text-center text-xs text-[var(--color-text-muted)]">
              No results found
            </div>
          )}

          {/* Focus Groups section */}
          {focusGroupResults.length > 0 && (
            <>
              <div className="px-4 pt-3 pb-1 text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">
                Focus Groups
              </div>
              {focusGroupResults.map((result, i) => {
                const globalIndex = i
                const isSelected = globalIndex === selectedIndex
                return (
                  <div
                    key={`fg-${result.id}`}
                    className="flex items-center gap-3 px-4 py-2 cursor-pointer"
                    style={{
                      background: isSelected ? 'var(--color-accent)' : 'transparent',
                      color: isSelected ? '#fff' : 'var(--color-text-primary)'
                    }}
                    onClick={() => selectResult(result)}
                    onMouseEnter={() => setSelectedIndex(globalIndex)}
                  >
                    {/* Group icon */}
                    <div
                      className="w-6 h-6 flex items-center justify-center flex-shrink-0"
                      style={{
                        border: `1px solid ${result.color || 'var(--color-border)'}`,
                        background: result.color ? `${result.color}18` : 'var(--color-bg)'
                      }}
                    >
                      <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
                        <path
                          d="M1 3.5A1.5 1.5 0 0 1 2.5 2h3.172a1.5 1.5 0 0 1 1.06.44L8.15 3.856a.5.5 0 0 0 .354.147H13.5A1.5 1.5 0 0 1 15 5.5v7a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 1 12.5v-9Z"
                          fill={result.color || 'var(--color-text-muted)'}
                          opacity="0.7"
                        />
                      </svg>
                    </div>
                    <div className="flex-1 min-w-0">
                      <span className="text-xs truncate block">{result.name}</span>
                    </div>
                    <span
                      className="text-[10px] flex-shrink-0"
                      style={{ color: isSelected ? 'rgba(255,255,255,0.7)' : 'var(--color-text-muted)' }}
                    >
                      {result.projectCount} workspace{result.projectCount !== 1 ? 's' : ''}
                    </span>
                  </div>
                )
              })}
            </>
          )}

          {/* Projects section */}
          {projectResults.length > 0 && (
            <>
              <div className="px-4 pt-3 pb-1 text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">
                Workspaces
              </div>
              {projectResults.map((result, i) => {
                const globalIndex = projectIndexOffset + i
                const isSelected = globalIndex === selectedIndex
                return (
                  <div
                    key={`proj-${result.id}`}
                    className="flex items-center gap-3 px-4 py-2 cursor-pointer"
                    style={{
                      background: isSelected ? 'var(--color-accent)' : 'transparent',
                      color: isSelected ? '#fff' : 'var(--color-text-primary)'
                    }}
                    onClick={() => selectResult(result)}
                    onMouseEnter={() => setSelectedIndex(globalIndex)}
                  >
                    <ProjectAvatar
                      projectPath={result.path}
                      projectName={result.name}
                      projectColor={result.color}
                      projectId={result.id}
                      iconUrl={result.iconUrl}
                      size={24}
                    />
                    <div className="flex-1 min-w-0">
                      <span className="text-xs truncate block">{result.name}</span>
                      <span
                        className="text-[10px] truncate block"
                        style={{ color: isSelected ? 'rgba(255,255,255,0.5)' : 'var(--color-text-muted)' }}
                      >
                        {result.path}
                      </span>
                    </div>
                  </div>
                )
              })}
            </>
          )}
        </div>

        {/* Footer hint */}
        <div
          className="px-4 py-2 border-t border-[var(--color-border)] flex items-center gap-4 text-[10px] text-[var(--color-text-muted)]"
        >
          <span>
            <kbd className="px-1 py-0.5 border border-[var(--color-border)] mr-1" style={{ background: 'var(--color-bg)' }}>&uarr;</kbd>
            <kbd className="px-1 py-0.5 border border-[var(--color-border)] mr-1" style={{ background: 'var(--color-bg)' }}>&darr;</kbd>
            navigate
          </span>
          <span>
            <kbd className="px-1 py-0.5 border border-[var(--color-border)] mr-1" style={{ background: 'var(--color-bg)' }}>&crarr;</kbd>
            select
          </span>
        </div>
      </div>
    </div>
  )
}
