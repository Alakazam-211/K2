// @vitest-environment jsdom
//
// #683 (0.39.39) — daemon-owned pinned-chat lifecycle, RENDERER cutover.
// `.k2so/prds/daemon-owned-pinned-chat.md`.
//
// Verifies the renderer's reduced role against the FROZEN ensure-pinned-chat
// contract:
//   - mount  → POST ensure-pinned-chat {project} → attach (TerminalPane
//              renders under the canonical key, NO command/args)
//   - refresh → ensure {forceRespawn:true}
//   - dropdown switch → set-chat-session then ensure {forceRespawn:true}
//   - SessionRemoved(this workspace) → idle pane, NO auto-respawn
//   - SessionAdded(this workspace)  → re-attach
//   - capability gate: when daemon-pinned-chat is UNSUPPORTED, the legacy
//     renderer-orchestrated path runs (resume-chat-args + breaker wiring).
//
// All daemon/Tauri/terminal boundaries are mocked so the component graph
// imports inert under jsdom. TerminalPane is replaced by a probe that
// records the props the cutover cares about (attachAgentName, command,
// args, onChildExit).

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react'
import { useEffect } from 'react'

// ── Hoisted spies / controllable state ───────────────────────────────────

const h = vi.hoisted(() => {
  // Typed mock signatures so `.mock.calls` carries the (route, body) args
  // — avoids the zero-arg-tuple TS errors a bare `vi.fn(async () => …)`
  // produces when its calls are indexed.
  const daemonCliPost = vi.fn<(route: string, body?: unknown) => Promise<unknown>>(
    async () => ({
      sessionId: 'sess-1',
      claudeSessionId: 'claude-1',
      resumedExisting: true,
      command: 'claude',
      args: ['--resume', 'claude-1'],
      cols: 120,
      rows: 40,
      reused: false,
    }),
  )
  const daemonCliGet = vi.fn<(route: string, params?: unknown) => Promise<unknown>>(
    async (route: string) => {
      if (route === 'chat/list') return []
      if (route === 'workspace/resume-chat-args') {
        return { command: 'claude', args: ['--resume', 'claude-legacy'], cwd: '/ws', resumeSession: 'claude-legacy', resumedExisting: true }
      }
      return undefined
    },
  )
  return {
    supported: { value: true },
    daemonCliPost,
    daemonCliGet,
    // session-events subscription handlers captured here so tests can fire
    // SessionAdded / SessionRemoved at will.
    sessionHandlers: { current: null as null | {
      onAdded?: (e: unknown) => void
      onRemoved?: (e: unknown) => void
    } },
    unsubscribe: vi.fn(),
    // Probe TerminalPane props.
    terminalProps: { current: null as null | Record<string, unknown> },
    // #689 — TerminalPane mount counter. A remount (key bump) unmounts +
    // mounts a fresh instance, incrementing this. A no-op (no key change)
    // leaves it untouched. Lets the tests prove the attachNonce guard.
    terminalMountCount: { value: 0 },
  }
})

vi.mock('@/lib/server-capabilities', () => ({
  useServerSupports: () => h.supported.value,
  serverSupports: () => h.supported.value,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: h.daemonCliPost,
  daemonCliGet: h.daemonCliGet,
}))

vi.mock('@/stores/session-events', () => ({
  subscribeToWorkspaceSessionEvents: (_path: string, handlers: unknown) => {
    h.sessionHandlers.current = handlers as never
    return h.unsubscribe
  },
}))

vi.mock('@/terminal-v2/TerminalPane', () => ({
  TerminalPane: (props: Record<string, unknown>) => {
    h.terminalProps.current = props
    // Count mounts so a test can detect a remount (key bump). Empty deps →
    // fires once per mounted instance; a key-driven remount creates a new
    // instance and fires again.
    // eslint-disable-next-line react-hooks/rules-of-hooks
    useEffect(() => {
      h.terminalMountCount.value += 1
    }, [])
    return (
      <div
        data-testid="terminal-pane"
        data-attach={String(props.attachAgentName)}
        data-command={props.command === undefined ? 'NONE' : String(props.command)}
        data-args={props.args ? JSON.stringify(props.args) : 'NONE'}
        data-has-onexit={props.onChildExit ? 'yes' : 'no'}
      />
    )
  },
}))

// Stores / libs the component graph touches at import-time.
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (selector: (s: unknown) => unknown) =>
    selector({ projects: [{ id: 'proj-1', path: '/ws' }] }),
}))
vi.mock('@/stores/active-agents', () => ({
  useActiveAgentsStore: { getState: () => ({ bindPaneProject: vi.fn() }) },
}))
vi.mock('@/stores/tabs', () => ({
  useTabsStore: { getState: () => ({ stampAgentSessionId: vi.fn() }) },
}))
vi.mock('@/lib/terminal-id', () => ({
  agentChatId: (pid: string, agent: string) => `agent-chat:${pid}:${agent}`,
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalExists: vi.fn(async () => false),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 1, token: 't', host: '127.0.0.1' })),
  daemonHttpBase: () => 'http://127.0.0.1:1',
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => 'Agent One'),
  resumeChatArgs: vi.fn(async () => ({ command: 'claude', args: ['--resume', 'claude-legacy'], cwd: '/ws', resumeSession: 'claude-legacy', resumedExisting: true })),
  reconcileColdBootSession: (hint: string, c: { resumeSession?: string; resumedExisting?: boolean; args?: string[] } | null) => {
    if (!c) return { kind: 'fallback', sessionId: hint }
    if (c.resumedExisting) return { kind: 'resume', sessionId: c.resumeSession || hint }
    return { kind: 'fresh', args: c.args ?? [], sessionId: c.resumeSession || hint }
  },
}))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => undefined),
}))

import { AgentChatPane } from './AgentChatPane'

beforeEach(() => {
  cleanup()
  h.supported.value = true
  h.daemonCliPost.mockClear()
  h.daemonCliGet.mockClear()
  h.unsubscribe.mockClear()
  h.sessionHandlers.current = null
  h.terminalProps.current = null
  h.terminalMountCount.value = 0
})

function ensureBodies(): Array<Record<string, unknown>> {
  return h.daemonCliPost.mock.calls
    .filter((c) => c[0] === 'workspace/ensure-pinned-chat')
    .map((c) => (c[1] ?? {}) as Record<string, unknown>)
}

function lastEnsureCall(): Record<string, unknown> | null {
  const bodies = ensureBodies()
  return bodies.length === 0 ? null : bodies[bodies.length - 1]
}

describe('#683 daemon-owned path — mount → ensure-pinned-chat → attach', () => {
  it('calls ensure-pinned-chat on mount (no forceRespawn) and attaches TerminalPane with NO command/args', async () => {
    render(<AgentChatPane agentName="agent" projectPath="/ws" />)

    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())

    const ensure = lastEnsureCall()
    expect(ensure).not.toBeNull()
    expect(ensure?.project).toBe('/ws')
    expect(ensure?.forceRespawn).toBeUndefined()

    const pane = screen.getByTestId('terminal-pane')
    // Attaches under the canonical workspace key (bare projectId).
    expect(pane.getAttribute('data-attach')).toBe('proj-1')
    // The renderer no longer builds claude args — daemon already spawned.
    expect(pane.getAttribute('data-command')).toBe('NONE')
    expect(pane.getAttribute('data-args')).toBe('NONE')
    // No breaker wiring on the daemon-owned path.
    expect(pane.getAttribute('data-has-onexit')).toBe('no')
  })

  it('forwards restoredSessionId to ensure ONLY as an offline hint', async () => {
    render(<AgentChatPane agentName="agent" projectPath="/ws" restoredSessionId="hint-9" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    const body = lastEnsureCall()
    expect(body?.restoredSessionId).toBe('hint-9')
  })
})

describe('#683 daemon-owned path — refresh & switch issue forceRespawn', () => {
  it('refresh button → ensure {forceRespawn:true}', async () => {
    render(<AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.daemonCliPost.mockClear()

    fireEvent.click(screen.getByLabelText('Refresh chat session'))

    await waitFor(() => {
      expect(lastEnsureCall()?.forceRespawn).toBe(true)
    })
  })

  it('dropdown switch → set-chat-session (persist) THEN ensure {forceRespawn:true}', async () => {
    // chat/list returns one other session so the dropdown has an option.
    h.daemonCliGet.mockImplementation(async (route: string): Promise<unknown> => {
      if (route === 'chat/list') {
        return [{ sessionId: 'other-sess', title: 'Older chat', timestamp: 2, messageCount: 3, provider: 'claude' }]
      }
      return undefined
    })
    render(<AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.daemonCliPost.mockClear()
    h.daemonCliGet.mockClear()

    // open the dropdown, click the other session.
    fireEvent.click(screen.getByLabelText('Switch pinned chat session'))
    await waitFor(() => expect(screen.queryByText('Older chat')).not.toBeNull())
    fireEvent.click(screen.getByText('Older chat'))

    await waitFor(() => {
      // persisted via set-chat-session
      const setCall = h.daemonCliGet.mock.calls.find((c) => c[0] === 'workspace/set-chat-session')
      expect(setCall).toBeTruthy()
      const params = (setCall?.[1] ?? {}) as { session_id?: string }
      expect(params.session_id).toBe('other-sess')
      // then a forceRespawn ensure
      expect(lastEnsureCall()?.forceRespawn).toBe(true)
    })
  })
})

describe('#683 daemon-owned path — broadcast re-attach / idle', () => {
  it('SessionRemoved(this workspace) → idle pane, NO auto-respawn', async () => {
    render(<AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.daemonCliPost.mockClear()

    // Daemon observed the child exit → broadcasts SessionRemoved.
    h.sessionHandlers.current!.onRemoved!({ agent_name: 'proj-1', workspace_path: '/ws' })

    await waitFor(() => {
      expect(screen.queryByTestId('terminal-pane')).toBeNull()
      expect(screen.queryByText('Chat session ended')).not.toBeNull()
    })
    // CRITICAL: no auto-respawn — the daemon never auto-spawns a pinned chat.
    expect(h.daemonCliPost.mock.calls.filter((c) => c[0] === 'workspace/ensure-pinned-chat')).toHaveLength(0)
  })

  it('ignores SessionRemoved for a DIFFERENT workspace key', async () => {
    render(<AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())

    h.sessionHandlers.current!.onRemoved!({ agent_name: 'some-other-proj', workspace_path: '/ws' })

    // Still attached — the event wasn't ours.
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    expect(screen.queryByText('Chat session ended')).toBeNull()
  })

  it('SessionAdded(this workspace) re-attaches after going idle (daemon respawn)', async () => {
    render(<AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())

    h.sessionHandlers.current!.onRemoved!({ agent_name: 'proj-1', workspace_path: '/ws' })
    await waitFor(() => expect(screen.queryByText('Chat session ended')).not.toBeNull())

    // Daemon respawned (e.g. k2so msg find-or-spawn) → SessionAdded.
    h.sessionHandlers.current!.onAdded!({
      agent_name: 'proj-1',
      workspace_path: '/ws',
      session_id: 'sess-2',
    })

    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
  })

  it('unsubscribes from session events on unmount', async () => {
    const { unmount } = render(<AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    unmount()
    expect(h.unsubscribe).toHaveBeenCalled()
  })
})

// ── #689 — remount guard: don't react to your own SessionAdded echo ───────
//
// Bug: mount → ensure(false) → daemon emits SessionAdded for the session we
// JUST ensured → onAdded bumped attachNonce → TerminalPane REMOUNTED (the
// 1–3s flicker + tab icon/label reset). Fix: track the attached session id;
// a SessionAdded echoing it is a no-op. A DIFFERENT session id (or a
// forceRespawn we initiated) DOES re-attach.

describe('#689 remount guard — SessionAdded echo is a no-op', () => {
  it('SessionAdded for the ALREADY-ATTACHED session does NOT remount TerminalPane', async () => {
    render(<AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    // mount = ensure(false) → ready with sessionId 'sess-1' (the mock).
    // TerminalPane mounted exactly once.
    expect(h.terminalMountCount.value).toBe(1)

    // The daemon echoes SessionAdded for the SAME session we just ensured.
    h.sessionHandlers.current!.onAdded!({
      agent_name: 'proj-1',
      workspace_path: '/ws',
      session_id: 'sess-1',
    })

    // No remount — still the original instance (no flicker, icon stable).
    await Promise.resolve()
    expect(h.terminalMountCount.value).toBe(1)
    expect(screen.queryByTestId('terminal-pane')).not.toBeNull()
  })

  it('SessionAdded for a DIFFERENT session DOES re-attach (remount)', async () => {
    render(<AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    expect(h.terminalMountCount.value).toBe(1)

    // A genuine change — the daemon respawned on a new session.
    h.sessionHandlers.current!.onAdded!({
      agent_name: 'proj-1',
      workspace_path: '/ws',
      session_id: 'sess-DIFFERENT',
    })

    // Remount happened (attachNonce bumped) → fresh TerminalPane instance.
    await waitFor(() => expect(h.terminalMountCount.value).toBe(2))
  })

  it('refresh (forceRespawn) re-attaches, and the new session’s echo is then a no-op', async () => {
    // mount returns sess-1; the refresh ensure returns sess-2.
    render(<AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    expect(h.terminalMountCount.value).toBe(1)

    h.daemonCliPost.mockResolvedValueOnce({
      sessionId: 'sess-2',
      claudeSessionId: 'claude-2',
      resumedExisting: false,
      command: 'claude',
      args: [],
      cols: 120,
      rows: 40,
      reused: false,
    })
    fireEvent.click(screen.getByLabelText('Refresh chat session'))

    // forceRespawn bumped attachNonce → remount onto the new PTY.
    await waitFor(() => expect(h.terminalMountCount.value).toBe(2))

    // The daemon's SessionAdded echo for sess-2 (the one we just respawned)
    // must NOT remount again — ensure() already stamped the ref to sess-2.
    h.sessionHandlers.current!.onAdded!({
      agent_name: 'proj-1',
      workspace_path: '/ws',
      session_id: 'sess-2',
    })
    await Promise.resolve()
    expect(h.terminalMountCount.value).toBe(2)
  })
})

describe('#683 capability gate — fallback to legacy renderer-orchestrated path', () => {
  it('when daemon-pinned-chat is UNSUPPORTED, runs the legacy path (resume-chat-args + breaker wiring, NO ensure)', async () => {
    h.supported.value = false
    render(<AgentChatPane agentName="agent" projectPath="/ws" restoredSessionId="hint-1" />)

    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())

    // Legacy path NEVER calls ensure-pinned-chat.
    expect(h.daemonCliPost.mock.calls.filter((c) => c[0] === 'workspace/ensure-pinned-chat')).toHaveLength(0)

    const pane = screen.getByTestId('terminal-pane')
    // Legacy path resolves claude args in the renderer (cold-boot resume).
    expect(pane.getAttribute('data-command')).toBe('claude')
    expect(pane.getAttribute('data-args')).not.toBe('NONE')
    // Breaker is wired ONLY on the fallback path.
    expect(pane.getAttribute('data-has-onexit')).toBe('yes')
  })
})
