import { describe, it, expect } from 'vitest'
import {
  resolveChatHistoryHost,
  type ResolvableProject,
} from './resolveHost'

// Two workspaces: K2 (globally active, running heartbeats) and HK47.
// Issue #7: opening HK47's history panel showed K2's chats because the
// component resolved from the GLOBAL active pointers (which pointed at
// K2) instead of the host workspace the panel was mounted inside.
const K2: ResolvableProject = {
  id: 'proj-k2',
  path: '/repos/k2',
  workspaces: [
    { id: 'ws-k2-main', branch: 'main', worktreePath: null },
    { id: 'ws-k2-feat', branch: 'feat/x', worktreePath: '/repos/k2-feat' },
  ],
}
const HK47: ResolvableProject = {
  id: 'proj-hk47',
  path: '/repos/hk47',
  workspaces: [{ id: 'ws-hk47-main', branch: 'main', worktreePath: null }],
}
const PROJECTS = [K2, HK47]

describe('resolveChatHistoryHost', () => {
  it('binds to the HOST project by path even when another project is globally active (#7)', () => {
    // Global pointers say K2 is active, but the panel is mounted in HK47.
    const r = resolveChatHistoryHost(PROJECTS, '/repos/hk47', 'proj-k2', 'ws-k2-main')
    expect(r.project?.id).toBe('proj-hk47')
    expect(r.projectPath).toBe('/repos/hk47')
    // Crucially NOT K2.
    expect(r.project?.id).not.toBe('proj-k2')
  })

  it('resolves a worktree workspace by matching worktreePath', () => {
    const r = resolveChatHistoryHost(PROJECTS, '/repos/k2-feat', 'proj-hk47', 'ws-hk47-main')
    expect(r.project?.id).toBe('proj-k2')
    expect(r.workspace?.id).toBe('ws-k2-feat')
    expect(r.workspace?.branch).toBe('feat/x')
    expect(r.projectPath).toBe('/repos/k2-feat')
  })

  it('resolves the main (non-worktree) workspace when host path equals project path', () => {
    const r = resolveChatHistoryHost(PROJECTS, '/repos/k2', null, null)
    expect(r.project?.id).toBe('proj-k2')
    // Main workspace has worktreePath === null.
    expect(r.workspace?.id).toBe('ws-k2-main')
    expect(r.workspace?.worktreePath).toBeNull()
  })

  it('still scopes the daemon call to the host path when no project row is loaded yet', () => {
    const r = resolveChatHistoryHost([], '/repos/not-loaded', 'proj-k2', 'ws-k2-main')
    expect(r.project).toBeNull()
    expect(r.workspace).toBeNull()
    // The daemon call must still target the host path (daemon filtering is
    // project-family aware) rather than leaking the globally-active path.
    expect(r.projectPath).toBe('/repos/not-loaded')
  })

  it('falls back to global pointers when no host path is supplied (legacy behavior)', () => {
    const r = resolveChatHistoryHost(PROJECTS, undefined, 'proj-k2', 'ws-k2-feat')
    expect(r.project?.id).toBe('proj-k2')
    expect(r.workspace?.id).toBe('ws-k2-feat')
    expect(r.projectPath).toBe('/repos/k2-feat')
  })

  it('fallback prefers worktreePath, then project path, then undefined', () => {
    // Main workspace active → project path.
    expect(
      resolveChatHistoryHost(PROJECTS, undefined, 'proj-hk47', 'ws-hk47-main').projectPath,
    ).toBe('/repos/hk47')
    // Nothing active → undefined (blank panel).
    expect(
      resolveChatHistoryHost(PROJECTS, undefined, null, null).projectPath,
    ).toBeUndefined()
  })
})
