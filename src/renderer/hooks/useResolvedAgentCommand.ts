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
 * Per-workspace scope (the project whose future `defaultAgent` field — Slice 1
 * — overrides the global setting) is picked, in order, from:
 *   1. `workspaceId` — the project containing that workspace
 *   2. `opts.projectPath` — the project rooted at that path (or owning a
 *      worktree at it)
 *   3. the active project
 * Until Slice 1 lands the field is absent, so every scope resolves to the
 * global `defaultAgent` setting (id-first, legacy-token tolerant), falling
 * back to the first enabled preset. Returns null only when no preset is
 * enabled (e.g. presets not yet fetched).
 */
export function useResolvedAgentCommand(
  workspaceId?: string,
  opts?: { projectPath?: string },
): ResolvedAgentCommand<AgentPreset> | null {
  const presets = usePresetsStore((s) => s.presets)
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const projectPath = opts?.projectPath

  return useMemo(() => {
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
  }, [presets, defaultAgent, projects, activeProjectId, workspaceId, projectPath])
}
