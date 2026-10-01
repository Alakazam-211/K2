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
import { act, render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react'
import ContextMenu from '@/components/ContextMenu/ContextMenu'
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
      if (route === 'chat/custom-names') return {}
      if (route === 'workspace/resume-chat-args') {
        return { command: 'claude', args: ['--resume', 'claude-legacy'], cwd: '/ws', resumeSession: 'claude-legacy', resumedExisting: true }
      }
      if (route === 'thread') return { ok: true, conversation_id: 'claude-1', items: [] }
      if (route === 'workspace/handle') return { handle: 'sales' }
      if (route === 'sessions/list-for-workspace') {
        return [{ kind: 'canonical', handle: 'sales', agentName: 'proj-1' }]
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
    // Pinned-chat retention — controllable canonical-Active mirror. The
    // daemon-owned path derives `retainWhileHidden` from membership here.
    activeIds: { value: new Set<string>() },
    // Slice 4 — host-aware set-chat-session client (workspace-agent.ts).
    // The dropdown switch persists the picked session id + PROVIDER
    // through this, no longer via a raw daemonCliGet call.
    setChatSession: vi.fn<(project: string, sessionId: string, provider?: string) => Promise<void>>(
      async () => undefined,
    ),
    resumeChatArgs: vi.fn(async () => ({
      command: 'claude',
      args: ['--resume', 'claude-legacy'],
      cwd: '/ws',
      resumeSession: 'claude-legacy',
      resumedExisting: true,
    })),
    stampAgentSessionId: vi.fn(),
    addTabToGroup: vi.fn(() => 'pane-should-not-exist'),
  }
})

vi.mock('@/lib/server-capabilities', () => ({
  useServerSupports: () => h.supported.value,
  serverSupports: () => h.supported.value,
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliPost: primaryOnly(h.daemonCliPost),
    daemonCliGet: primaryOnly(h.daemonCliGet),
  }
})

vi.mock('@/stores/session-events', async () => {
  // Home M1: every subscription takes the server scope first; these
  // mocks fail loudly unless it is the primary scope.
  const { expectPrimaryScope } = await import('@/test-utils/scope')
  return {
    subscribeToWorkspaceSessionEvents: (scope: unknown, _path: string, handlers: unknown) => {
      expectPrimaryScope(scope)
      h.sessionHandlers.current = handlers as never
      return h.unsubscribe
    },
    onChatHistoryChanged: () => () => {},
  }
})

vi.mock('@/kessel-term/TerminalPane', () => ({
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
        data-sessionid={props.sessionId === undefined ? 'NONE' : String(props.sessionId)}
        data-has-onexit={props.onChildExit ? 'yes' : 'no'}
        data-retain={String(props.retainWhileHidden === true)}
      >
        <div data-testid="message-compose" data-compose-bar="" />
      </div>
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
// Pinned-chat retention — mocked (rather than the real tiny store) because
// the real module imports `@/stores/settings`, whose module-init fetch graph
// is far heavier than this component test needs.
vi.mock('@/stores/active', () => ({
  useActiveStore: (selector: (s: { activeProjectIds: Set<string> }) => unknown) =>
    selector({ activeProjectIds: h.activeIds.value }),
}))
vi.mock('@/stores/tabs', () => ({
  useTabsStore: {
    getState: () => ({
      stampAgentSessionId: h.stampAgentSessionId,
      addTabToGroup: h.addTabToGroup,
    }),
  },
  registerPresetsStore: () => {},
  closeV2Session: vi.fn(async () => undefined),
}))
vi.mock('@/lib/terminal-id', () => ({
  agentChatId: (pid: string, agent: string) => `agent-chat:${pid}:${agent}`,
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalExists: vi.fn(async () => false),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 1, token: 't', host: '127.0.0.1', secure: false })),
  daemonHttpBase: () => 'http://127.0.0.1:1',
  daemonWsBase: () => 'ws://127.0.0.1:1',
}))

vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: Object.assign(
    (sel: (s: { activeHost: 'local' }) => unknown) => sel({ activeHost: 'local' }),
    { getState: () => ({ activeHost: 'local' as const }) },
  ),
  activeHostKey: () => 'local',
  // settings.ts registers a host-switch listener at module scope; Thread
  // overlay pulls that store in through this pane.
  onActiveHostChange: () => () => {},
}))
vi.mock('@/lib/workspace-agent', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
  agentDisplayName: vi.fn(async () => 'Agent One'),
  setChatSession: primaryOnly(h.setChatSession),
  resumeChatArgs: primaryOnly(h.resumeChatArgs),
  // Mirrors the Slice-4 shape: resume/fresh carry the daemon's
  // command+args verbatim (per-harness grammar).
  reconcileColdBootSession: (hint: string, c: { command?: string; resumeSession?: string; resumedExisting?: boolean; args?: string[] } | null) => {
    if (!c) return { kind: 'fallback', sessionId: hint }
    if (c.resumedExisting) return { kind: 'resume', sessionId: c.resumeSession || hint, command: c.command ?? 'claude', args: c.args ?? [] }
    return { kind: 'fresh', sessionId: c.resumeSession || hint, command: c.command ?? 'claude', args: c.args ?? [] }
  },
  }
})
// Slice 4 — provider icon stub: renders an inspectable marker instead of
// dragging the AgentIcon svg-asset graph into jsdom.
vi.mock('@/components/AgentIcon/ProviderIcon', () => ({
  ProviderIcon: ({ provider }: { provider: string }) => (
    <span data-testid="provider-icon" data-provider={provider} />
  ),
}))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => undefined),
}))

class FakeWS {
  url: string
  onmessage: ((ev: { data: string }) => void) | null = null
  constructor(url: string) {
    this.url = url
  }
  close() {}
}
vi.stubGlobal('WebSocket', FakeWS)

import { AgentChatPane } from './AgentChatPane'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { useTabsStore } from '@/stores/tabs'

// Home M3 — the pane reads its room: the mocked tabs store, this suite's
// project list, and an activity sink (MS68). The scope is the primary one,
// so the `primaryOnly` request mocks still hold.
const bindPaneProject = vi.fn()
const room = testRoom({
  tabs: useTabsStore,
  projects: [{ id: 'proj-1', path: '/ws', workspaces: [] } as never],
  activity: { bindPaneProject },
})

beforeEach(() => {
  cleanup()
  h.supported.value = true
  h.daemonCliPost.mockClear()
  h.daemonCliGet.mockClear()
  h.setChatSession.mockClear()
  h.resumeChatArgs.mockClear()
  h.stampAgentSessionId.mockClear()
  h.addTabToGroup.mockClear()
  h.unsubscribe.mockClear()
  h.sessionHandlers.current = null
  h.terminalProps.current = null
  h.terminalMountCount.value = 0
  h.activeIds.value = new Set()
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
  // A workspace visit mounts this pane; seedBoot / Active-list warming
  // must not. Visit still POSTs ensure-pinned-chat
  // (prd-active-window-wake-and-reap-v1 §3.4 / test 8).
  it('calls ensure-pinned-chat on mount (no forceRespawn) and attaches TerminalPane with NO command/args', async () => {
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)

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
    // R6/R18: ensure response sessionId is forwarded (eager marker, not attach-only).
    expect(pane.getAttribute('data-sessionid')).toBe('sess-1')
    // No breaker wiring on the daemon-owned path.
    expect(pane.getAttribute('data-has-onexit')).toBe('no')
  })

  it('forwards restoredSessionId to ensure ONLY as an offline hint', async () => {
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" restoredSessionId="hint-9" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    const body = lastEnsureCall()
    expect(body?.restoredSessionId).toBe('hint-9')
  })
})

describe('pinned-chat retention — retainWhileHidden threading', () => {
  it('workspace in the canonical Active set → TerminalPane gets retainWhileHidden=true', async () => {
    h.activeIds.value = new Set(['proj-1'])
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    expect(screen.getByTestId('terminal-pane').getAttribute('data-retain')).toBe('true')
  })

  it('workspace NOT in the Active set → retainWhileHidden=false (park-on-hidden preserved)', async () => {
    h.activeIds.value = new Set(['some-other-proj'])
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    expect(screen.getByTestId('terminal-pane').getAttribute('data-retain')).toBe('false')
  })
})

describe('#683 daemon-owned path — refresh & switch issue forceRespawn', () => {
  it('refresh button → ensure {forceRespawn:true}', async () => {
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
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
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.daemonCliPost.mockClear()
    h.daemonCliGet.mockClear()

    // open the dropdown, click the other session.
    fireEvent.click(screen.getByLabelText('Switch pinned chat session'))
    await waitFor(() => expect(screen.queryByText('Older chat')).not.toBeNull())
    fireEvent.click(screen.getByText('Older chat'))

    await waitFor(() => {
      // persisted via the setChatSession client (id + provider)
      expect(h.setChatSession).toHaveBeenCalledWith('/ws', 'other-sess', 'claude')
      // then a forceRespawn ensure
      expect(lastEnsureCall()?.forceRespawn).toBe(true)
    })
  })
})

// ── Slice 4 (agent-degeneralization) — multi-agent canonical dropdown ─────
//
// The picker lists EVERY provider's sessions for the workspace (the
// claude-only filter is gone), renders a provider mark per row, and a
// pick persists the row's provider alongside the id so the daemon stamps
// workspace_sessions.harness and respawns in that harness's grammar.

describe('Slice 4 — multi-agent canonical-session dropdown', () => {
  const MIXED_ROWS = [
    { sessionId: 'claude-sess', title: 'Claude chat', timestamp: 3, messageCount: 5, provider: 'claude' },
    { sessionId: 'pi-sess', title: 'Pi chat', timestamp: 2, messageCount: 4, provider: 'pi' },
    { sessionId: 'codex-sess', title: 'Codex chat', timestamp: 1, messageCount: 2, provider: 'codex' },
  ]

  it('lists ALL providers’ sessions with a provider icon per row (no claude filter)', async () => {
    h.daemonCliGet.mockImplementation(async (route: string): Promise<unknown> => {
      if (route === 'chat/list') return MIXED_ROWS
      if (route === 'chat/custom-names') return {}
      return undefined
    })
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())

    fireEvent.click(screen.getByLabelText('Switch pinned chat session'))
    await waitFor(() => expect(screen.queryByText('Pi chat')).not.toBeNull())

    // Every provider's session renders — the old filter dropped pi/codex.
    expect(screen.queryByText('Claude chat')).not.toBeNull()
    expect(screen.queryByText('Codex chat')).not.toBeNull()

    // Each row carries its provider's icon.
    const icons = screen.getAllByTestId('provider-icon')
    expect(icons.map((el) => el.getAttribute('data-provider'))).toEqual([
      'claude', 'pi', 'codex',
    ])
  })

  it('shows the current session’s provider icon on the closed dropdown trigger', async () => {
    h.daemonCliGet.mockImplementation(async (route: string): Promise<unknown> => {
      if (route === 'chat/list') {
        return [
          { sessionId: 'claude-1', title: 'Current', timestamp: 9, messageCount: 3, provider: 'pi' },
          ...MIXED_ROWS,
        ]
      }
      if (route === 'chat/custom-names') return {}
      return undefined
    })
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())

    const trigger = screen.getByLabelText('Switch pinned chat session')
    await waitFor(() => {
      const icon = trigger.querySelector('[data-testid="provider-icon"]')
      expect(icon).not.toBeNull()
      expect(icon?.getAttribute('data-provider')).toBe('pi')
    })
  })

  it('picking a NON-claude row persists its provider via set-chat-session then respawns', async () => {
    h.daemonCliGet.mockImplementation(async (route: string): Promise<unknown> => {
      if (route === 'chat/list') return MIXED_ROWS
      if (route === 'chat/custom-names') return {}
      return undefined
    })
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.daemonCliPost.mockClear()

    fireEvent.click(screen.getByLabelText('Switch pinned chat session'))
    await waitFor(() => expect(screen.queryByText('Pi chat')).not.toBeNull())
    fireEvent.click(screen.getByText('Pi chat'))

    await waitFor(() => {
      expect(h.setChatSession).toHaveBeenCalledWith('/ws', 'pi-sess', 'pi')
      // The respawn goes through ensure-pinned-chat — the DAEMON resolves
      // the stored harness's command; the renderer sends no command/args.
      expect(lastEnsureCall()?.forceRespawn).toBe(true)
    })
    expect(screen.getByTestId('terminal-pane').getAttribute('data-command')).toBe('NONE')
    expect(screen.getByTestId('terminal-pane').getAttribute('data-args')).toBe('NONE')
  })

  it('rows without a provider (older daemon) degrade to claude', async () => {
    h.daemonCliGet.mockImplementation(async (route: string): Promise<unknown> => {
      if (route === 'chat/list') {
        return [{ sessionId: 'legacy-sess', title: 'Legacy chat', timestamp: 1, messageCount: 1 }]
      }
      if (route === 'chat/custom-names') return {}
      return undefined
    })
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())

    fireEvent.click(screen.getByLabelText('Switch pinned chat session'))
    await waitFor(() => expect(screen.queryByText('Legacy chat')).not.toBeNull())
    expect(screen.getByTestId('provider-icon').getAttribute('data-provider')).toBe('claude')

    fireEvent.click(screen.getByText('Legacy chat'))
    await waitFor(() => {
      expect(h.setChatSession).toHaveBeenCalledWith('/ws', 'legacy-sess', 'claude')
    })
  })
})

describe('#683 daemon-owned path — broadcast re-attach / idle', () => {
  it('SessionRemoved(this workspace) → idle pane, NO auto-respawn', async () => {
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
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
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())

    h.sessionHandlers.current!.onRemoved!({ agent_name: 'some-other-proj', workspace_path: '/ws' })

    // Still attached — the event wasn't ours.
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    expect(screen.queryByText('Chat session ended')).toBeNull()
  })

  it('SessionAdded(this workspace) re-attaches after going idle (daemon respawn)', async () => {
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
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
    const { unmount } = renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
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
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    // waitFor the mount counter, not just the DOM node: the probe increments
    // in useEffect, which can lag the first paint under full-suite load.
    await waitFor(() => expect(h.terminalMountCount.value).toBe(1))

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
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(h.terminalMountCount.value).toBe(1))

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
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(h.terminalMountCount.value).toBe(1))

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
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" restoredSessionId="hint-1" />)

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

describe('S2 overlay chrome (C3/C4/C10)', () => {
  it('defaults to Terminal and keeps ChatHeader dropdown on Thread without unmounting PTY', async () => {
    renderInRoom(room, 
      <>
        <AgentChatPane agentName="agent" projectPath="/ws" />
        <ContextMenu />
      </>,
    )
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())

    expect(screen.getByTestId('session-view-menu').getAttribute('data-view')).toBe('terminal')
    expect(screen.queryByTestId('session-view-tabs')).toBeNull()
    expect(screen.queryByTestId('session-view-chat')).toBeNull()
    expect(screen.getByLabelText('Switch pinned chat session')).not.toBeNull()
    expect(screen.getByTestId('pinned-chat-header')).not.toBeNull()
    const header = screen.getByTestId('pinned-chat-header')
    const menu = screen.getByTestId('session-view-menu')
    expect(menu.parentElement?.parentElement?.parentElement).toBe(header)
    const rowKids = Array.from(menu.parentElement!.children)
    expect(rowKids.indexOf(menu)).toBe(0)

    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-button'))
    })
    expect(screen.getByTestId('session-view-menu').getAttribute('data-view')).toBe('terminal')
    const thread = document.querySelector('[data-context-menu] button')
    expect(thread).toBeTruthy()
    const threadRow = Array.from(document.querySelectorAll('[data-context-menu] button')).find((button) =>
      Array.from(button.querySelectorAll('span')).some((span) => span.textContent === 'Thread'),
    )
    expect(threadRow).toBeTruthy()
    await act(async () => {
      fireEvent.click(threadRow!)
    })

    expect(screen.getByTestId('session-view-menu').getAttribute('data-view')).toBe('thread')
    expect(screen.getByTestId('terminal-pane')).not.toBeNull()
    expect(screen.getByTestId('message-compose')).not.toBeNull()
    expect(screen.queryByTestId('thread-compose')).toBeNull()
    expect(h.terminalProps.current).not.toBeNull()
    expect(h.terminalProps.current!.showComposeBar).toBe(true)
    expect(screen.getByLabelText('Switch pinned chat session')).not.toBeNull()
    expect(screen.getByLabelText('Refresh chat session')).not.toBeNull()
  })
})

const SOURCE = {
  sessionId: 'claude-1',
  title: 'Finish the editor',
  timestamp: 9,
  messageCount: 12,
  provider: 'claude',
}
const OTHER = {
  sessionId: 'grok-older',
  title: 'Notes from Monday',
  timestamp: 1,
  messageCount: 4,
  provider: 'grok',
}
const PREMINT = '11111111-2222-4333-8444-555555555555'

function mountEnsure(over: Record<string, unknown> = {}) {
  return {
    sessionId: 'sess-1',
    claudeSessionId: 'claude-1',
    resumedExisting: true,
    command: 'claude',
    args: ['--resume', 'claude-1'],
    cols: 80,
    rows: 24,
    reused: false,
    provider: 'claude',
    ...over,
  }
}

function freshEnsure(over: Record<string, unknown> = {}) {
  return {
    sessionId: 'pty-new',
    claudeSessionId: PREMINT,
    resumedExisting: false,
    command: 'claude',
    args: ['--dangerously-skip-permissions', '--session-id', PREMINT],
    cols: 80,
    rows: 24,
    reused: false,
    provider: 'claude',
    pendingSessionDiscovery: false,
    freshSpawn: true,
    ...over,
  }
}

function delivered() {
  return {
    success: true,
    target_session_id: 'pty-new',
    attempts: 1,
    reason: null,
    hint: null,
  }
}

describe('Continue in a new chat — pinned session switcher', () => {
  function listRows(extra: Array<Record<string, unknown>> = []) {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'chat/list') return [SOURCE, OTHER, ...extra]
      if (route === 'chat/custom-names') return {}
      if (route === 'workspace/handle') return { handle: 'sales' }
      if (route === 'sessions/list-for-workspace') {
        return [{ kind: 'canonical', handle: 'sales', agentName: 'proj-1' }]
      }
      return undefined
    })
  }

  async function openSwitcher(): Promise<void> {
    fireEvent.click(screen.getByLabelText('Switch pinned chat session'))
    await screen.findByTestId('pinned-continue-new-chat')
  }

  function continueRow(): HTMLButtonElement {
    return screen.getByTestId('pinned-continue-new-chat') as HTMLButtonElement
  }

  it('pins Continue in a new chat… above the rows, including when the list is empty', async () => {
    listRows()
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    await openSwitcher()

    const row = continueRow()
    const list = screen.getByTestId('pinned-session-list')
    expect(row.textContent).toBe('Continue in a new chat…')
    expect(list.contains(row)).toBe(false)
    expect(list.className).toContain('overflow-y-auto')
    expect(list.className).toContain('border-t')
    expect(row.compareDocumentPosition(list) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(list.textContent).toContain('Finish the editor')
    expect(list.textContent).toContain('Notes from Monday')
    expect(screen.getByLabelText('Refresh chat session')).not.toBeNull()
    expect(row.parentElement?.contains(screen.getByLabelText('Refresh chat session'))).toBe(false)

    cleanup()
    listRows()
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'chat/list') return []
      if (route === 'chat/custom-names') return {}
      return undefined
    })
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    await openSwitcher()
    const emptyList = screen.getByTestId('pinned-session-list')
    const again = continueRow()
    expect(emptyList.textContent).toContain('No past sessions yet.')
    expect(emptyList.contains(again)).toBe(false)
    expect(again.compareDocumentPosition(emptyList) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
  })

  it('disables a null or empty provider id, and still opens for a premint missing from the list', async () => {
    listRows()
    h.daemonCliPost.mockRejectedValueOnce(new Error('ensure failed'))
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await screen.findByLabelText('Switch pinned chat session')
    await openSwitcher()
    expect(continueRow().disabled).toBe(true)
    expect(continueRow().getAttribute('title')).toBe('No chat to continue.')
    fireEvent.click(continueRow())
    expect(screen.queryByTestId('continue-new-chat')).toBeNull()

    cleanup()
    listRows()
    h.daemonCliPost.mockResolvedValueOnce(mountEnsure({ claudeSessionId: '', pendingSessionDiscovery: true, resumedExisting: false, args: [] }))
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    await openSwitcher()
    expect(screen.getByLabelText('Switch pinned chat session').textContent).toContain('New chat')
    expect(continueRow().disabled).toBe(true)
    expect(continueRow().getAttribute('title')).toBe('No chat to continue.')
    fireEvent.click(continueRow())
    expect(screen.queryByTestId('continue-new-chat')).toBeNull()

    cleanup()
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'chat/list') return [OTHER]
      if (route === 'chat/custom-names') return {}
      return undefined
    })
    h.daemonCliPost.mockResolvedValueOnce(mountEnsure({
      claudeSessionId: PREMINT,
      resumedExisting: false,
      args: ['--dangerously-skip-permissions', '--session-id', PREMINT],
    }))
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    await openSwitcher()
    expect(screen.getByLabelText('Switch pinned chat session').textContent).toContain('New chat')
    expect(continueRow().disabled).toBe(false)
    expect(continueRow().getAttribute('title')).not.toBe('No chat to continue.')
    fireEvent.click(continueRow())
    expect(await screen.findByTestId('continue-new-chat')).toBeTruthy()
    expect(screen.getByTestId('continue-new-chat').querySelector('select')).toBeNull()
  })

  it('opens the shared dialog without switching, seeding, or adding a tab', async () => {
    listRows()
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.daemonCliPost.mockClear()
    h.setChatSession.mockClear()
    h.addTabToGroup.mockClear()
    await openSwitcher()
    fireEvent.click(continueRow())
    const dialog = await screen.findByTestId('continue-new-chat')
    expect(dialog.textContent).toContain('Finish the editor')
    expect(dialog.textContent).not.toContain('From Claude')
    expect(dialog.querySelector('[data-continue-source] svg')).toBeTruthy()
    expect(dialog.textContent).toContain('Claude')
    expect(dialog.querySelector('select')).toBeNull()
    expect((screen.getByLabelText('Harness') as HTMLElement).tagName).toBe('BUTTON')
    expect(h.setChatSession).not.toHaveBeenCalled()
    expect(h.addTabToGroup).not.toHaveBeenCalled()
    expect(h.daemonCliPost.mock.calls.filter((call) => call[0] === 'workspace/ensure-pinned-chat')).toHaveLength(0)
    expect(h.daemonCliPost.mock.calls.filter((call) => call[0] === 'chat/continue-seed')).toHaveLength(0)
  })

  it('Esc, Cancel, and the scrim do not seed or send', async () => {
    listRows()
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'chat/continue-seed') return { text: 'SEED TEXT' }
      if (route === 'terminal/send-message') return delivered()
      return mountEnsure()
    })
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.daemonCliPost.mockClear()

    await openSwitcher()
    fireEvent.click(continueRow())
    await screen.findByTestId('continue-new-chat')
    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByTestId('continue-new-chat')).toBeNull())
    expect(h.daemonCliPost).not.toHaveBeenCalled()

    await openSwitcher()
    fireEvent.click(continueRow())
    await screen.findByTestId('continue-new-chat')
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByTestId('continue-new-chat')).toBeNull())
    expect(h.daemonCliPost).not.toHaveBeenCalled()

    await openSwitcher()
    fireEvent.click(continueRow())
    const frame = await screen.findByTestId('continue-new-chat')
    const scrim = frame.previousElementSibling
    expect(scrim).toBeTruthy()
    fireEvent.mouseDown(scrim as Element)
    await waitFor(() => expect(screen.queryByTestId('continue-new-chat')).toBeNull())
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    expect(h.addTabToGroup).not.toHaveBeenCalled()
  })

  it('Start seeds the pinned chat, fresh-spawns, and sends to ensure’s PTY id', async () => {
    listRows([{ sessionId: PREMINT, title: 'Continued', timestamp: 20, messageCount: 0, provider: 'claude' }])
    h.daemonCliPost.mockImplementation(async (route: string, body?: unknown) => {
      if (route === 'chat/continue-seed') return { text: 'SEED TEXT from the daemon' }
      if (route === 'terminal/send-message') return delivered()
      const posted = (body ?? {}) as Record<string, unknown>
      if (route === 'workspace/ensure-pinned-chat' && posted.freshProvider) return freshEnsure()
      return mountEnsure()
    })
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.daemonCliPost.mockClear()
    h.setChatSession.mockClear()
    h.addTabToGroup.mockClear()
    await openSwitcher()
    fireEvent.click(continueRow())
    await screen.findByTestId('continue-new-chat')
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))

    await waitFor(() => {
      expect(h.daemonCliPost.mock.calls.some((call) => call[0] === 'chat/continue-seed')).toBe(true)
    })
    const seed = h.daemonCliPost.mock.calls.find((call) => call[0] === 'chat/continue-seed')
    expect(seed?.[1]).toEqual({
      provider: 'claude',
      sessionId: 'claude-1',
      projectPath: '/ws',
      mode: 'recent',
      targetProvider: 'claude',
    })
    const ensured = h.daemonCliPost.mock.calls.find((call) => {
      const body = call[1] as Record<string, unknown> | undefined
      return call[0] === 'workspace/ensure-pinned-chat' && body?.freshProvider
    })
    expect(ensured?.[1]).toEqual({ project: '/ws', freshProvider: 'claude' })
    expect(h.addTabToGroup).not.toHaveBeenCalled()

    await waitFor(() => {
      expect(h.daemonCliPost.mock.calls.some((call) => call[0] === 'terminal/send-message')).toBe(true)
    })
    const send = h.daemonCliPost.mock.calls.find((call) => call[0] === 'terminal/send-message')
    expect(send?.[1]).toEqual({
      session_id: 'pty-new',
      text: 'SEED TEXT from the daemon\n\nPrevious session file: /ws',
    })
    await waitFor(() => expect(h.setChatSession).toHaveBeenCalledWith('/ws', PREMINT, 'claude'))
    expect(h.setChatSession.mock.calls.some((call) => call[1] === 'claude-1')).toBe(false)
    expect(h.stampAgentSessionId).toHaveBeenCalledWith('agent', '/ws', PREMINT, 'proj-1')
    await waitFor(() => expect(screen.queryByTestId('continue-new-chat')).toBeNull())

    await openSwitcher()
    const selected = screen.getByRole('option', { selected: true })
    expect(selected.textContent).toContain('Continued')
    const sourceRow = screen.getByRole('option', { name: /Finish the editor/ })
    expect(sourceRow.getAttribute('aria-selected')).toBe('false')
    expect(screen.getByRole('option', { name: /Notes from Monday/ })).toBeTruthy()
  })

  it('keeps the dialog mounted through ensuring, and a pty_died leaves the source selected', async () => {
    listRows()
    let release: (value: unknown) => void = () => {}
    const gate = new Promise((resolve) => { release = resolve })
    h.daemonCliPost.mockImplementation(async (route: string, body?: unknown) => {
      if (route === 'chat/continue-seed') return { text: 'SEED TEXT' }
      if (route === 'terminal/send-message') {
        return { success: false, reason: 'pty_died', hint: 'the pty died', target_session_id: 'pty-new', attempts: 1 }
      }
      const posted = (body ?? {}) as Record<string, unknown>
      if (route === 'workspace/ensure-pinned-chat' && posted.freshProvider) {
        await gate
        return freshEnsure()
      }
      return mountEnsure()
    })
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    await openSwitcher()
    fireEvent.click(continueRow())
    await screen.findByTestId('continue-new-chat')
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))
    await screen.findByText('Loading session…')
    expect(screen.getByTestId('continue-new-chat')).toBeTruthy()
    h.sessionHandlers.current!.onAdded!({
      agent_name: 'proj-1',
      workspace_path: '/ws',
      session_id: 'pty-uuid-not-a-chat',
    })
    release(undefined)
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('the pty died')
    expect(screen.getByTestId('continue-new-chat')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Copy text' })).toBeTruthy()
    expect(h.setChatSession).not.toHaveBeenCalled()
    await waitFor(() => {
      expect(screen.getByLabelText('Switch pinned chat session').textContent).toContain('Finish the editor')
    })
    expect(screen.queryByText('pty-uuid-not-a-chat')).toBeNull()
  })

  it('waits to persist a self-mint id until the new transcript appears', async () => {
    const rows: Array<Record<string, unknown>> = [SOURCE, OTHER]
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'chat/list') return rows.slice()
      if (route === 'chat/custom-names') return {}
      return undefined
    })
    h.daemonCliPost.mockImplementation(async (route: string, body?: unknown) => {
      if (route === 'chat/continue-seed') return { text: 'SEED TEXT' }
      if (route === 'terminal/send-message') return delivered()
      const posted = (body ?? {}) as Record<string, unknown>
      if (route === 'workspace/ensure-pinned-chat' && posted.freshProvider) {
        return freshEnsure({
          claudeSessionId: '',
          pendingSessionDiscovery: true,
          provider: 'codex',
          command: 'codex',
          args: ['--yolo'],
        })
      }
      return mountEnsure()
    })
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.setChatSession.mockClear()
    await openSwitcher()
    fireEvent.click(continueRow())
    await screen.findByTestId('continue-new-chat')
    fireEvent.click(screen.getByLabelText('Harness'))
    const menu = await screen.findByTestId('setting-dropdown-menu')
    const codex = Array.from(menu.querySelectorAll('button')).find((btn) =>
      btn.textContent?.includes('Codex'),
    )
    expect(codex).toBeTruthy()
    fireEvent.click(codex!)
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))
    await waitFor(() => expect(screen.queryByTestId('continue-new-chat')).toBeNull())
    expect(h.setChatSession).not.toHaveBeenCalled()
    expect(h.addTabToGroup).not.toHaveBeenCalled()
    const ensured = h.daemonCliPost.mock.calls.find((call) => {
      const body = call[1] as Record<string, unknown> | undefined
      return call[0] === 'workspace/ensure-pinned-chat' && body?.freshProvider === 'codex'
    })
    expect(ensured?.[1]).toEqual({ project: '/ws', freshProvider: 'codex' })

    rows.push({ sessionId: 'codex-new', title: 'Codex continued', timestamp: 30, messageCount: 1, provider: 'codex' })
    await waitFor(() => expect(h.setChatSession).toHaveBeenCalledWith('/ws', 'codex-new', 'codex'), { timeout: 3000 })
    expect(h.setChatSession.mock.calls.some((call) => call[1] === 'claude-1')).toBe(false)
    await openSwitcher()
    expect(screen.getByRole('option', { selected: true }).textContent).toContain('Codex continued')
    expect(screen.getByRole('option', { name: /Finish the editor/ }).getAttribute('aria-selected')).toBe('false')
  })

  it('a daemon without freshSpawn shows the error and does not send or resume', async () => {
    listRows()
    h.daemonCliPost.mockImplementation(async (route: string, body?: unknown) => {
      if (route === 'chat/continue-seed') return { text: 'SEED TEXT' }
      if (route === 'terminal/send-message') return delivered()
      const posted = (body ?? {}) as Record<string, unknown>
      if (posted.freshProvider) {
        return mountEnsure()
      }
      return mountEnsure()
    })
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.daemonCliPost.mockClear()
    h.resumeChatArgs.mockClear()
    h.setChatSession.mockClear()
    await openSwitcher()
    fireEvent.click(continueRow())
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('cannot continue a pinned chat in place')
    expect(screen.getByTestId('continue-new-chat')).toBeTruthy()
    expect(h.daemonCliPost.mock.calls.some((call) => call[0] === 'terminal/send-message')).toBe(false)
    expect(h.resumeChatArgs).not.toHaveBeenCalled()
    expect(h.setChatSession).not.toHaveBeenCalled()
    expect(h.addTabToGroup).not.toHaveBeenCalled()
    const body = h.daemonCliPost.mock.calls.find((call) => call[0] === 'workspace/ensure-pinned-chat')?.[1] as Record<string, unknown>
    expect(body.forceRespawn).toBeUndefined()
    expect(body.explicitSelection).toBeUndefined()
    expect(body.freshProvider).toBe('claude')
  })

  it('legacy Start does not refresh or resume, and the dialog survives Loading session…', async () => {
    h.supported.value = false
    let resumeCalls = 0
    h.resumeChatArgs.mockImplementation(async () => {
      resumeCalls += 1
      if (resumeCalls > 1) await new Promise(() => {})
      return {
        command: 'claude',
        args: ['--resume', 'claude-legacy'],
        cwd: '/ws',
        resumeSession: 'claude-legacy',
        resumedExisting: true,
      }
    })
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'chat/continue-seed') return { text: 'SEED TEXT' }
      if (route === 'terminal/send-message') return delivered()
      return mountEnsure()
    })
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'chat/list') return [{ ...SOURCE, sessionId: 'claude-legacy' }]
      if (route === 'chat/custom-names') return {}
      if (route === 'workspace/resume-chat-args') {
        return {
          command: 'claude',
          args: ['--resume', 'claude-legacy'],
          cwd: '/ws',
          resumeSession: 'claude-legacy',
          resumedExisting: true,
        }
      }
      return undefined
    })
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.resumeChatArgs.mockClear()
    h.daemonCliPost.mockClear()
    await openSwitcher()
    fireEvent.click(continueRow())
    await screen.findByTestId('continue-new-chat')
    fireEvent.click(screen.getByLabelText('Refresh chat session'))
    await screen.findByText('Loading session…')
    expect(screen.getByTestId('continue-new-chat')).toBeTruthy()

    cleanup()
    h.supported.value = false
    resumeCalls = 0
    h.resumeChatArgs.mockImplementation(async () => ({
      command: 'claude',
      args: ['--resume', 'claude-legacy'],
      cwd: '/ws',
      resumeSession: 'claude-legacy',
      resumedExisting: true,
    }))
    renderInRoom(room, <AgentChatPane agentName="agent" projectPath="/ws" />)
    await waitFor(() => expect(screen.queryByTestId('terminal-pane')).not.toBeNull())
    h.resumeChatArgs.mockClear()
    h.daemonCliPost.mockClear()
    h.addTabToGroup.mockClear()
    await openSwitcher()
    fireEvent.click(continueRow())
    await screen.findByTestId('continue-new-chat')
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('cannot continue a pinned chat in place')
    expect(h.resumeChatArgs).not.toHaveBeenCalled()
    expect(h.daemonCliPost.mock.calls.some((call) => call[0] === 'workspace/ensure-pinned-chat')).toBe(false)
    expect(h.daemonCliPost.mock.calls.some((call) => call[0] === 'terminal/send-message')).toBe(false)
    expect(h.addTabToGroup).not.toHaveBeenCalled()
    expect(screen.getByTestId('continue-new-chat')).toBeTruthy()
  })
})
