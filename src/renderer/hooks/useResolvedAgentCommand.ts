// Agent de-generalization Slice 0 — thin React wrapper around the pure
// resolution seam (@/lib/agent-resolve). Components should use this instead
// of hand-matching `defaultAgent` against the presets list.
import { useMemo } from 'react'
import { usePresetsStore, type AgentPreset } from '@/stores/presets'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import {
  resolveAgentCommand,
  readProjectDefaultAgent,
  type ResolvedAgentCommand,
} from '@/lib/agent-resolve'

export type { ResolvedAgentCommand }

/**
 * Resolve the effective default agent as `{ preset, command, args }`.
 *
 * Per-workspace scope (the project `defaultAgent` field) is picked, in order,
 * from:
 *   1. `workspaceId` — the project containing that workspace
 *   2. `opts.projectPath` — the project rooted at that path (or owning a
 *      worktree at it)
 *   3. the active project
 * Then: workspace override → global `defaultAgent` → first enabled preset.
 *
 * Pass `opts.scope: 'global'` to ignore workspace overrides (AI File Editor
 * and other Settings helpers follow Editors & Agents → Default AI Agent).
 * ⇧⌘T / new tabs keep the default `'workspace'` scope.
 */
export function useResolvedAgentCommand(
  workspaceId?: string,
  opts?: { projectPath?: string; scope?: 'workspace' | 'global' },
): ResolvedAgentCommand<AgentPreset> | null {
  const presets = usePresetsStore((s) => s.presets)
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const projectPath = opts?.projectPath
  const scope = opts?.scope ?? 'workspace'

  return useMemo(() => {
    if (scope === 'global') {
      return resolveAgentCommand(presets, defaultAgent, undefined)
    }

    const project =
      (workspaceId
        ? projects.find((p) => p.workspaces.some((w) => w.id === workspaceId))
        : undefined) ??
      (projectPath
        ? projects.find(
            (p) =>
              p.path === projectPath ||
              p.workspaces.some((w) => w.worktreePath === projectPath),
          )
        : undefined) ??
      projects.find((p) => p.id === activeProjectId)

    return resolveAgentCommand(presets, defaultAgent, readProjectDefaultAgent(project))
  }, [presets, defaultAgent, projects, activeProjectId, workspaceId, projectPath, scope])
}
