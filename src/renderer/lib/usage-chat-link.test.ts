// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { canOpenUsageChat, findUsageWorkspace, usageResumeLaunch } from './usage-chat-link'

const PROJECTS = [
  { id: 'p1', path: '/w/k2', workspaces: [{ id: 'ws1', worktreePath: null }, { id: 'ws2', worktreePath: '/w/k2/.worktrees/feat' }] },
  { id: 'p2', path: '/private/tmp/scratch', workspaces: [{ id: 'ws3', worktreePath: null }] },
]

describe('findUsageWorkspace', () => {
  it('matches a worktree first, then a project path (first workspace), with /tmp aliases', () => {
    expect(findUsageWorkspace(PROJECTS, '/w/k2/.worktrees/feat')).toEqual({ projectId: 'p1', workspaceId: 'ws2', cwd: '/w/k2/.worktrees/feat' })
    expect(findUsageWorkspace(PROJECTS, '/w/k2/')).toEqual({ projectId: 'p1', workspaceId: 'ws1', cwd: '/w/k2' })
    expect(findUsageWorkspace(PROJECTS, '/tmp/scratch')).toEqual({ projectId: 'p2', workspaceId: 'ws3', cwd: '/private/tmp/scratch' })
    expect(findUsageWorkspace(PROJECTS, '/elsewhere')).toBeNull()
    expect(findUsageWorkspace(PROJECTS, null)).toBeNull()
  })
})

describe('usageResumeLaunch', () => {
  it('resumes in each harness grammar', () => {
    expect(usageResumeLaunch('claude', 'abc')).toEqual({ command: 'claude', args: ['--resume', 'abc'] })
    expect(usageResumeLaunch('codex', 'abc')).toEqual({ command: 'codex', args: ['resume', 'abc'] })
    expect(usageResumeLaunch('grok', 'abc')).toEqual({ command: 'grok', args: ['--resume', 'abc'] })
    expect(usageResumeLaunch('unknown-cli', 'abc')).toBeNull()
  })
})

describe('canOpenUsageChat', () => {
  it('needs a session, a known workspace and a resumable harness', () => {
    expect(canOpenUsageChat(PROJECTS, { workspace: '/w/k2', session_id: 's', harness: 'claude' })).toBe(true)
    expect(canOpenUsageChat(PROJECTS, { workspace: '/w/k2', session_id: '', harness: 'claude' })).toBe(false)
    expect(canOpenUsageChat(PROJECTS, { workspace: null, session_id: 's', harness: 'claude' })).toBe(false)
    expect(canOpenUsageChat(PROJECTS, { workspace: '/w/k2', session_id: 's', harness: 'mystery' })).toBe(false)
  })
})
