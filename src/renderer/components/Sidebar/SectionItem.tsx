import { useCallback } from 'react'
import { useProjectsStore, type WorkspaceSection } from '@/stores/projects'
import { useGitInfo } from '@/hooks/useGit'
import { showContextMenu } from '@/lib/context-menu'

// ── Preset section colors ────────────────────────────────────────────────────

const SECTION_COLORS = [
  { label: 'Blue', value: '#3b82f6' },
  { label: 'Green', value: '#22c55e' },
  { label: 'Yellow', value: '#eab308' },
  { label: 'Orange', value: '#f97316' },
  { label: 'Red', value: '#ef4444' },
  { label: 'Purple', value: '#a855f7' },
  { label: 'Pink', value: '#ec4899' },
  { label: 'Cyan', value: '#06b6d4' }
]

// ── Worktree git badge ──────────────────────────────────────────────────────

function WorkspaceGitBadge({ path }: { path?: string }): React.JSX.Element | null {
  const { data } = useGitInfo(path)
  if (!data?.isRepo) return null

  const count = data.changedFiles + data.untrackedFiles
  if (count === 0) return null

  return (
    <span className="ml-auto text-[10px] tabular-nums font-medium px-1.5 py-0.5 bg-yellow-400/10 text-yellow-400 flex-shrink-0">
      {count}
    </span>
  )
}

// ── SectionItem ──────────────────────────────────────────────────────────────

interface SectionItemProps {
  section: WorkspaceSection
  workspaces: Array<{
    id: string
    projectId: string
    sectionId: string | null
    type: string
    branch: string | null
    name: string
    tabOrder: number
    worktreePath: string | null
    createdAt: number
  }>
  projectPath: string
  activeWorkspaceId: string | null
  onWorkspaceClick: (workspaceId: string) => void
  onWorkspaceContextMenu: (e: React.MouseEvent, workspaceId: string) => void
}

export default function SectionItem({
  section,
  workspaces,
  projectPath,
  activeWorkspaceId,
  onWorkspaceClick,
  onWorkspaceContextMenu
}: SectionItemProps): React.JSX.Element {
  const updateSection = useProjectsStore((s) => s.updateSection)
  const deleteSection = useProjectsStore((s) => s.deleteSection)
  const renameSection = useProjectsStore((s) => s.renameSection)

  const isCollapsed = section.isCollapsed === 1

  const handleToggleCollapse = useCallback(() => {
    updateSection(section.id, { isCollapsed: isCollapsed ? 0 : 1 })
  }, [section.id, isCollapsed, updateSection])

  const handleContextMenu = useCallback(
    async (e: React.MouseEvent) => {
      e.preventDefault()
      e.stopPropagation()

      const colorItems = SECTION_COLORS.map((c) => ({
        id: `color:${c.value}`,
        label: `${c.label}${section.color === c.value ? ' *' : ''}`
      }))

      const menuItems = [
        { id: 'rename', label: 'Rename' },
        { id: 'separator-1', label: '', type: 'separator' as const },
        ...colorItems,
        { id: 'color-none', label: `None${!section.color ? ' *' : ''}` },
        { id: 'separator-2', label: '', type: 'separator' as const },
        { id: 'delete', label: 'Delete Section' }
      ]

      const clickedId = await showContextMenu(menuItems)

      if (clickedId === 'rename') {
        const newName = window.prompt('Rename section:', section.name)
        if (newName && newName.trim() && newName !== section.name) {
          await renameSection(section.id, newName.trim())
        }
      } else if (clickedId === 'delete') {
        await deleteSection(section.id)
      } else if (clickedId === 'color-none') {
        await updateSection(section.id, { color: null })
      } else if (clickedId?.startsWith('color:')) {
        const color = clickedId.replace('color:', '')
        await updateSection(section.id, { color })
      }
    },
    [section, renameSection, deleteSection, updateSection]
  )

  return (
    <div>
      {/* Section header */}
      <button
        className="w-full flex items-center gap-1.5 px-2 py-1 text-left text-[10px] uppercase tracking-wider font-semibold text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors"
        style={{
          borderLeft: section.color ? `3px solid ${section.color}` : '3px solid transparent',
          width: 'calc(100% - 4px)'
        }}
        onClick={handleToggleCollapse}
        onContextMenu={handleContextMenu}
      >
        <svg
          className={`w-2.5 h-2.5 flex-shrink-0 transition-transform ${isCollapsed ? '' : 'rotate-90'}`}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2.5}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
        </svg>
        <span className="truncate flex-1">{section.name}</span>
        <span className="text-[9px] tabular-nums opacity-60 flex-shrink-0">
          {workspaces.length}
        </span>
      </button>

      {/* Child worktrees */}
      {!isCollapsed && (
        <div className="ml-1">
          {workspaces.map((workspace) => {
            const workspacePath = workspace.worktreePath ?? projectPath
            const isWorktree = workspace.type === 'worktree'

            return (
              <button
                key={workspace.id}
                className={`w-full flex items-center gap-2 px-3 py-1 text-left text-xs transition-colors ${
                  activeWorkspaceId === workspace.id
                    ? 'bg-[var(--color-accent)]/15 text-[var(--color-accent)]'
                    : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.04]'
                }`}
                style={{ width: 'calc(100% - 4px)' }}
                onClick={() => onWorkspaceClick(workspace.id)}
                onContextMenu={(e) => onWorkspaceContextMenu(e, workspace.id)}
              >
                {isWorktree ? (
                  <svg
                    className="w-3 h-3 flex-shrink-0 opacity-50"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke="currentColor"
                    strokeWidth={2}
                  >
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A2 2 0 013 12V7a4 4 0 014-4z"
                    />
                  </svg>
                ) : (
                  <svg
                    className="w-3 h-3 flex-shrink-0 opacity-50"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke="currentColor"
                    strokeWidth={2}
                  >
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      d="M13 10V3L4 14h7v7l9-11h-7z"
                    />
                  </svg>
                )}

                <span className="truncate">{workspace.name}</span>

                {isWorktree && workspace.branch && (
                  <span className="text-[10px] text-[var(--color-text-muted)] truncate flex-shrink-0 max-w-[80px]">
                    {workspace.branch}
                  </span>
                )}

                <WorkspaceGitBadge path={workspacePath} />
              </button>
            )
          })}

          {workspaces.length === 0 && (
            <div className="px-4 py-1.5 text-[10px] text-[var(--color-text-muted)] opacity-50 italic">
              No worktrees
            </div>
          )}
        </div>
      )}
    </div>
  )
}

export { SECTION_COLORS }
