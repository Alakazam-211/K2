// Plan B (Bulk-2) — vitest coverage for the workspace primary-agent client
// after migrating the 3 daemon-proxy commands (k2so_workspace_agent_display_name,
// k2so_workspace_set_agent_display_name, k2so_agents_resume_chat_args) OFF
// the Tauri invoke proxy ONTO the host-aware `daemonCliGet` HTTP layer.
//
// Routes (all GET, confirmed against crates/k2so-daemon/src/cli.rs):
//   GET workspace/agent-display-name?project=     → {display_name}  (snake)
//   GET workspace/set-agent-display-name?project=&name=  (GET mutation)
//   GET workspace/resume-chat-args?project=       → camelCase ResumeChatArgs

import { describe, it, expect, beforeEach, vi } from 'vitest'

const daemonCliGet = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
}))

import {
  agentDisplayName,
  setAgentDisplayName,
  resumeChatArgs,
  reconcileColdBootSession,
  type ResumeChatArgs,
} from './workspace-agent'

describe('workspace-agent — Plan B host-aware migration', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
  })

  it('agentDisplayName GETs the snake_case route and unwraps display_name', async () => {
    daemonCliGet.mockResolvedValueOnce({ display_name: 'manager' })
    const name = await agentDisplayName('/work/proj')
    expect(daemonCliGet).toHaveBeenCalledWith('workspace/agent-display-name', {
      project: '/work/proj',
    })
    expect(name).toBe('manager')
  })

  it('agentDisplayName returns "" when display_name is absent', async () => {
    daemonCliGet.mockResolvedValueOnce({})
    expect(await agentDisplayName('/work/proj')).toBe('')
  })

  it('setAgentDisplayName GETs the mutation route with project + name', async () => {
    daemonCliGet.mockResolvedValueOnce({ success: true })
    await setAgentDisplayName('/work/proj', 'lead')
    expect(daemonCliGet).toHaveBeenCalledWith('workspace/set-agent-display-name', {
      project: '/work/proj',
      name: 'lead',
    })
  })

  it('resumeChatArgs GETs the camelCase route and returns the payload as-is', async () => {
    const payload = {
      command: 'claude',
      args: ['--resume', 'sid-1'],
      cwd: '/work/proj',
      resumeSession: 'sid-1',
    }
    daemonCliGet.mockResolvedValueOnce(payload)
    const result = await resumeChatArgs('/work/proj')
    expect(daemonCliGet).toHaveBeenCalledWith('workspace/resume-chat-args', {
      project: '/work/proj',
    })
    expect(result).toEqual(payload)
  })

  it('propagates a daemon error (display-name read)', async () => {
    daemonCliGet.mockRejectedValueOnce(new Error('daemon down'))
    await expect(agentDisplayName('/work/proj')).rejects.toThrow('daemon down')
  })
})

describe('reconcileColdBootSession (GH#679 revive + GH#681 no-phantom-resume)', () => {
  const ca = (over: Partial<ResumeChatArgs>): ResumeChatArgs => ({
    command: 'claude',
    args: [],
    cwd: '/work/proj',
    ...over,
  })

  it('RESUMES the SQLite session when it is a different resumable conversation (GH#679)', () => {
    // The dropdown wrote `sqlite-new` to workspace_sessions, but the layout
    // JSON still carries the pre-switch `layout-old`. On cold boot SQLite wins.
    const result = reconcileColdBootSession(
      'layout-old',
      ca({ resumeSession: 'sqlite-new', resumedExisting: true }),
    )
    expect(result).toEqual({ kind: 'resume', sessionId: 'sqlite-new' })
  })

  it('RESUMES the layout hint when SQLite matches it (no-op, real conversation)', () => {
    const result = reconcileColdBootSession(
      'same-sid',
      ca({ resumeSession: 'same-sid', resumedExisting: true }),
    )
    expect(result).toEqual({ kind: 'resume', sessionId: 'same-sid' })
  })

  it('GH#681: does NOT --resume when SQLite pre-allocated a FRESH UUID (resumedExisting=false)', () => {
    // Brand-new / never-chatted workspace: the daemon minted a new UUID and
    // returned `--session-id <new>`. We must spawn those args verbatim, NOT
    // --resume a session that has no conversation on disk.
    const result = reconcileColdBootSession(
      'pre-allocated-uuid',
      ca({
        resumeSession: 'pre-allocated-uuid',
        resumedExisting: false,
        args: ['--dangerously-skip-permissions', '--session-id', 'pre-allocated-uuid'],
      }),
    )
    expect(result).toEqual({
      kind: 'fresh',
      args: ['--dangerously-skip-permissions', '--session-id', 'pre-allocated-uuid'],
      sessionId: 'pre-allocated-uuid',
    })
    // Hard guard: the resulting args must never contain --resume.
    if (result.kind === 'fresh') {
      expect(result.args).not.toContain('--resume')
      expect(result.args).toContain('--session-id')
    }
  })

  it('falls back to RESUMING the layout hint when the daemon read failed (null canonical)', () => {
    expect(reconcileColdBootSession('layout-old', null)).toEqual({
      kind: 'fallback',
      sessionId: 'layout-old',
    })
  })
})
