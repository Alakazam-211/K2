import { useState, useCallback } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { usePanelsStore } from '@/stores/panels'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { showContextMenu } from '@/lib/context-menu'
import { useGitInfo } from '@/hooks/useGit'

interface FocusWorkspaceHeaderProps {
  side: 'left' | 'right'
}

export default function FocusWorkspaceHeader({ side }: FocusWorkspaceHeaderProps): React.JSX.Element | null {
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const projects = useProjectsStore((s) => s.projects)
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)
  const moveFocusWorkspaceHeader = usePanelsStore((s) => s.moveFocusWorkspaceHeader)

  const [worktreeExpanded, setWorktreeExpanded] = useState(true)

  const oppositeSide = side === 'left' ? 'right' : 'left'
  const oppositeLabel = oppositeSide === 'left' ? 'Left' : 'Right'

  const handleContextMenu = useCallback(async (e: React.MouseEvent) => {
    e.preventDefault()
    const clickedId = await showContextMenu([
      { id: 'move', label: `Move to ${oppositeLabel} Panel` },
    ])
    if (clickedId === 'move') {
      moveFocusWorkspaceHeader(oppositeSide)
      const store = usePanelsStore.getState()
      if (oppositeSide === 'left' && !store.leftPanelOpen) store.toggleLeftPanel()
      if (oppositeSide === 'right' && !store.rightPanelOpen) store.toggleRightPanel()
    }
  }, [oppositeSide, oppositeLabel, moveFocusWorkspaceHeader])

  const handleWorktreeClick = useCallback((workspaceId: string) => {
    const proj = useProjectsStore.getState().projects.find((p) => p.id === activeProjectId)
    if (proj) {
      setActiveWorkspace(proj.id, workspaceId)
    }
  }, [activeProjectId, setActiveWorkspace])

  const { data: gitInfo } = useGitInfo(
    projects.find((p) => p.id === activeProjectId)?.path
  )

  // All hooks above — safe to return null now
  const project = projects.find((p) => p.id === activeProjectId)
  if (!project) return null

  const hasWorktrees = project.worktreeMode === 1 && project.workspaces.length > 1
  const activeWorkspace = project.workspaces.find((w) => w.id === activeWorkspaceId)

  return (
    <div
      className="border-b border-[var(--color-border)] select-none"
      onContextMenu={handleContextMenu}
    >
      {/* Project header — click anywhere to toggle worktree drawer */}
      <div
        className={`flex items-stretch gap-2.5 px-3 py-2 ${hasWorktrees ? 'cursor-pointer hover:bg-white/[0.03]' : ''}`}
        onClick={hasWorktrees ? () => setWorktreeExpanded(!worktreeExpanded) : undefined}
      >
        <div className="flex items-center flex-shrink-0">
          <ProjectAvatar
            projectPath={project.path}
            projectName={project.name}
            projectColor={project.color}
            projectId={project.id}
            iconUrl={project.iconUrl}
            size={32}
          />
        </div>
        <div className="flex flex-col justify-center min-w-0 flex-1">
          <div className="text-sm font-medium text-[var(--color-text-primary)] truncate">
            {project.name}
          </div>
          {gitInfo?.isRepo && gitInfo.currentBranch && (
            <div className="text-[10px] text-[var(--color-text-muted)] font-mono truncate" title={gitInfo.currentBranch}>
              {gitInfo.currentBranch}
            </div>
          )}
        </div>

        {/* Chevron indicator */}
        {hasWorktrees && (
          <div className="flex items-center flex-shrink-0">
            <svg
              className={`w-3 h-3 text-[var(--color-text-muted)] transition-transform ${worktreeExpanded ? 'rotate-90' : ''}`}
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={2}
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <polyline points="9 18 15 12 9 6" />
            </svg>
          </div>
        )}
      </div>

      {/* Worktree list (collapsible) */}
      {hasWorktrees && worktreeExpanded && (
        <div className="pb-1">
          {project.workspaces.map((ws) => {
            const isActive = ws.id === activeWorkspaceId
            return (
              <button
                key={ws.id}
                className={`w-full flex items-center gap-2 px-3 py-1 text-left transition-colors no-drag ${
                  isActive
                    ? 'bg-[var(--color-accent)]/10 text-[var(--color-text-primary)]'
                    : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.03]'
                }`}
                onClick={() => handleWorktreeClick(ws.id)}
              >
                {/* Branch icon */}
                <svg className="w-3 h-3 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
                  <line x1="6" y1="3" x2="6" y2="15" />
                  <circle cx="18" cy="6" r="3" />
                  <circle cx="6" cy="18" r="3" />
                  <path d="M18 9a9 9 0 0 1-9 9" />
                </svg>
                <span className="text-[11px] font-mono truncate">
                  {ws.branch || ws.name}
                </span>
                {isActive && (
                  <span className="ml-auto w-1.5 h-1.5 bg-[var(--color-accent)] flex-shrink-0" />
                )}
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}
