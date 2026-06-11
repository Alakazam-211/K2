// #672 — the renderer dismiss-reap is GONE.
//
// Previously (#657) the renderer scheduled a 15s grace-reap that closed a
// dismissed workspace's pinned-Chat v2 session. Per
// .k2so/prds/daemon-canonical-active.md §4.5/§6 the DAEMON now owns the
// Active set AND the grace-reap; the renderer is a pure consumer of the
// canonical Active mirror and must NOT close v2 sessions for
// age-out/dismiss anymore.
//
// This suite locks the removal: the tabs store no longer exposes any reap
// scheduler / sweep / grace constant, so the renderer cannot schedule a
// reap. (The daemon-side reaper + its grace are covered by the daemon
// integration suite, not here.)

import { describe, it, expect, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the modules import) ─────────────────

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') {
      return { state: 'unavailable', reason: 'test env', port: null, token: null }
    }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => ''),
}))

import * as tabsModule from './tabs'
import { useTabsStore } from './tabs'

describe('#672 — renderer reaping is removed (daemon owns it now)', () => {
  it('the tabs store exposes no reap scheduler / sweep API', () => {
    const state = useTabsStore.getState() as unknown as Record<string, unknown>
    // The #657/#658 renderer reaper surface is gone.
    expect(state.scheduleWorkspaceChatReap).toBeUndefined()
    expect(state.cancelWorkspaceChatReap).toBeUndefined()
    expect(state.sweepAgedOutWorkspaceChats).toBeUndefined()
    expect(state.sweepAgedOutWorkspaceChatsFromDaemon).toBeUndefined()
  })

  it('the tabs module no longer exports the reaper constants / test hooks', () => {
    const mod = tabsModule as Record<string, unknown>
    expect(mod.DISMISS_REAP_GRACE_MS).toBeUndefined()
    expect(mod.__clearPendingChatReapsForTests).toBeUndefined()
    expect(mod.__hasPendingChatReapForTests).toBeUndefined()
    // The open/attach⇒activate wiring DID replace it (PRD §4.3.1).
    expect(typeof mod.registerActivateProject).toBe('function')
  })
})
