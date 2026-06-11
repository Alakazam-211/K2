import { useState, useCallback, useMemo } from 'react'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { useTabsStore } from '@/stores/tabs'
import { useGitInfo, useGitChanges } from '@/hooks/useGit'
import { useMergeDialogStore } from '@/components/MergeDialog/MergeDialog'
import { useContextMenuStore } from '@/stores/context-menu'
import WorktreeDialog from '@/components/Sidebar/WorktreeDialog'

// ── Git status badge (changed files count → AI Commit on hover) ───────────

function GitBadge({ path }: { path?: string }): React.JSX.Element | null {
  const { data } = useGitInfo(path)
  const { data: changes } = useGitChanges(path)
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const [hovered, setHovered] = useState(false)

  if (!data?.isRepo) return null

  const count = data.changedFiles + data.untrackedFiles
  if (count === 0) return null

  const handleAiCommit = (e: React.MouseEvent) => {
    e.stopPropagation()
    if (!path) return

    const preset = presets.find((p) => p.id === defaultAgent)
    if (!preset) return

    const { command, args } = parseCommand(preset.command)

    const MAX_FILES = 80
    const fileLines = changes.slice(0, MAX_FILES).map((f: { status: string; path: string }) => `${f.status}: ${f.path}`)
    if (changes.length > MAX_FILES) {
      fileLines.push(`...and ${changes.length - MAX_FILES} more files`)
    }

    const prompt = `Review the following changes in this repository and create a well-structured commit with an appropriate commit message.\n\nChanged files:\n${fileLines.join('\n')}`

    const tabsStore = useTabsStore.getState()
    const activeGroup = tabsStore.activeGroupIndex
    tabsStore.addTabToGroup(activeGroup, path, {
      title: 'AI Commit',
      command,
      args: [...args, prompt]
    })
  }

  return (
    <span
      className="ml-1 flex-shrink-0 cursor-pointer"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onClick={handleAiCommit}
    >
      {hovered ? (
        <span className="text-[10px] font-medium px-1 bg-[var(--color-accent)]/15 text-[var(--color-accent)] leading-none whitespace-nowrap">
          AI Commit
        </span>
      ) : (
        <span className="text-[10px] tabular-nums font-medium px-1 bg-yellow-400/10 text-yellow-400 leading-none">
          {count}
        </span>
      )}
    </span>
  )
}

// ── Branch icon SVG ───────────────────────────────────────────────────────

function BranchIcon(): React.JSX.Element {
  return (
    <svg
      className="w-3 h-3 flex-shrink-0 opacity-60"
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
  )
}

// ── WorktreeBar ───────────────────────────────────────────────────────────

interface WorktreeBarProps {
  project: ProjectWithWorkspaces
}

export default function WorktreeBar({ project }: WorktreeBarProps): React.JSX.Element {
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)
  const agentMap = useActiveAgentsStore((s) => s.agents)

  const [worktreeDialogOpen, setWorktreeDialogOpen] = useState(false)

  const handleWorkspaceClick = useCallback(
    (workspaceId: string) => {
      setActiveWorkspace(project.id, workspaceId)
    },
    [project.id, setActiveWorkspace]
  )

  const worktreeMode = project.worktreeMode === 1

  // Single worktree mode: just show the branch name
  if (!worktreeMode && project.workspaces.length <= 1) {
    const workspace = project.workspaces[0]
    const workspacePath = workspace?.worktreePath ?? project.path
    const hasActiveAgentSingle = Array.from(agentMap.values()).some(a => a.status === 'active')
    const hasIdleAgentSingle = agentMap.size > 0 && !hasActiveAgentSingle

    return (
      <div
        className="flex items-center h-[32px] min-h-[32px] px-3 bg-[var(--color-bg)] border-b border-[var(--color-border)] select-none no-drag"
      >
        <div className="flex items-center gap-1.5 text-xs text-[var(--color-text-muted)]">
          {(hasActiveAgentSingle || hasIdleAgentSingle) && (
            <span
              className={`flex-shrink-0 rounded-full ${hasActiveAgentSingle ? 'agent-active-dot' : ''}`}
              style={{ width: 5, height: 5, backgroundColor: hasActiveAgentSingle ? '#f97316' : '#22c55e' }}
            />
          )}
          <BranchIcon />
          <span className="font-mono">{workspace?.branch ?? 'main'}</span>
          {workspace && <GitBadge path={workspacePath} />}
        </div>
      </div>
    )
  }

  // Group worktrees by section for the horizontal bar
  const sections = project.sections || []
  const ungroupedWorkspaces = project.workspaces.filter((ws) => !ws.sectionId)
  const hasSections = sections.length > 0

  const showMergeDialog = useMergeDialogStore((s) => s.show)
  const showContextMenu = useContextMenuStore((s) => s.show)

  const handleWorktreeContextMenu = useCallback(
    async (e: React.MouseEvent, workspace: typeof project.workspaces[number]) => {
      e.preventDefault()
      if (workspace.type !== 'worktree' || !workspace.branch) return

      const selected = await showContextMenu(e.clientX, e.clientY, [
        { id: 'merge', label: `Merge "${workspace.branch}" into current branch` },
      ])

      if (selected === 'merge') {
        showMergeDialog(workspace.branch!, project.path, project.id, workspace.id)
      }
    },
    [project, showContextMenu, showMergeDialog]
  )

  const renderWorkspaceTab = (workspace: typeof project.workspaces[number]) => {
    const isActive = workspace.id === activeWorkspaceId
    const workspacePath = workspace.worktreePath ?? project.path
    // Only the active workspace has live terminals — others are suspended
    const hasActiveAgent = isActive && Array.from(agentMap.values()).some(a => a.status === 'active')
    const hasIdleAgent = isActive && agentMap.size > 0 && !hasActiveAgent

    return (
      <button
        key={workspace.id}
        className={`group flex items-center gap-1.5 h-full px-3 text-xs font-mono whitespace-nowrap transition-colors border-r border-[var(--color-border)] ${
          isActive
            ? 'bg-[var(--color-accent)]/10 text-[var(--color-accent)] border-b-2 border-b-[var(--color-accent)]'
            : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.03]'
        }`}
        onClick={() => handleWorkspaceClick(workspace.id)}
        onContextMenu={(e) => handleWorktreeContextMenu(e, workspace)}
      >
        {(hasActiveAgent || hasIdleAgent) && (
          <span
            className={`flex-shrink-0 rounded-full ${hasActiveAgent ? 'agent-active-dot' : ''}`}
            style={{ width: 5, height: 5, backgroundColor: hasActiveAgent ? '#f97316' : '#22c55e' }}
          />
        )}
        <BranchIcon />
        <span>{workspace.name}</span>
        {workspace.branch && workspace.branch !== workspace.name && (
          <span className="text-[10px] opacity-50">{workspace.branch}</span>
        )}
        <GitBadge path={workspacePath} />
        {/* Merge button — visible on hover for non-main worktrees */}
        {workspace.type === 'worktree' && workspace.branch && (
          <span
            className="opacity-0 group-hover:opacity-100 ml-0.5 hover:text-[var(--color-accent)] cursor-pointer"
            title={`Merge "${workspace.branch}" into current branch`}
            onClick={(e) => {
              e.stopPropagation()
              showMergeDialog(workspace.branch!, project.path, project.id, workspace.id)
            }}
          >
            <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M7 16V4m0 0L3 8m4-4l4 4m6 0v12m0 0l4-4m-4 4l-4-4" />
            </svg>
          </span>
        )}
      </button>
    )
  }

  return (
    <>
      <div
        className="flex items-center h-[32px] min-h-[32px] bg-[var(--color-bg)] border-b border-[var(--color-border)] select-none no-drag overflow-x-auto overflow-y-hidden"
      >
        <div className="flex items-center h-full">
          {/* Ungrouped worktrees */}
          {ungroupedWorkspaces.map(renderWorkspaceTab)}

          {/* Section groups */}
          {hasSections && sections.map((section) => {
            const sectionWorkspaces = project.workspaces.filter(
              (ws) => ws.sectionId === section.id
            )

            if (sectionWorkspaces.length === 0) return null

            return (
              <div key={section.id} className="flex items-center h-full">
                {/* Section label divider */}
                <div
                  className="flex items-center h-full px-2 text-[9px] uppercase tracking-wider font-semibold text-[var(--color-text-muted)] border-r border-[var(--color-border)] select-none"
                  style={{
                    borderLeft: section.color
                      ? `3px solid ${section.color}`
                      : '1px solid var(--color-border)',
                    background: 'rgba(255,255,255,0.02)'
                  }}
                >
                  {section.name}
                </div>
                {sectionWorkspaces.map(renderWorkspaceTab)}
              </div>
            )
          })}

          {/* Add worktree button */}
          {worktreeMode && (
            <button
              className="flex items-center justify-center h-full px-2.5 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.03] transition-colors"
              onClick={() => setWorktreeDialogOpen(true)}
              title="New worktree"
            >
              <svg
                className="w-3.5 h-3.5"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
              >
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
              </svg>
            </button>
          )}
        </div>
      </div>

      {worktreeDialogOpen && (
        <WorktreeDialog
          projectId={project.id}
          projectPath={project.path}
          open={true}
          onClose={() => setWorktreeDialogOpen(false)}
        />
      )}
    </>
  )
}
