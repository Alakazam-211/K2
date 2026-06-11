// P1.A — the pinned-Chat working-state must attribute the braille spinner
// to its OWN project, not to whatever workspace the user is viewing when
// the first 'working' title-signal fires.
//
// ROOT CAUSE (regression guard): `recordTitleActivity` bound a not-yet-
// mapped paneId to `useProjectsStore.activeProjectId`. The pinned Chat has
// no agent lifecycle hook, so this title path is its only binder — and it
// latched to the wrong project whenever the user was on a different
// workspace, so `getProjectStatus(itsOwnProject)` missed and no spinner
// lit. The pinned-Chat terminalId is `agent-chat:<projectId>`, so the
// binder now parses the paneId and uses ITS embedded projectId.
//
// vitest env is node (no Tauri / daemon). Mock the load-time boundaries so
// importing the store is inert, then drive the store directly.

import { describe, it, expect, beforeEach, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
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
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => ({})),
  settingsReset: vi.fn(async () => ({})),
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalCreate: vi.fn(),
  terminalExists: vi.fn(async () => false),
  terminalListRunning: vi.fn(async () => []),
}))

// The store reads `useProjectsStore.getState()` for activeProjectId +
// touchInteraction. Mock it so the test controls the "currently viewed"
// workspace and can assert touchInteraction's target.
const touchInteraction = vi.fn()
let activeProjectId: string | null = null
vi.mock('./projects', () => ({
  useProjectsStore: {
    getState: () => ({
      activeProjectId,
      touchInteraction,
      projects: [],
    }),
  },
}))

import { useActiveAgentsStore } from './active-agents'
import { agentChatId } from '@/lib/terminal-id'

function reset(): void {
  useActiveAgentsStore.setState({
    paneStatuses: new Map(),
    paneProjectMap: new Map(),
    agents: new Map(),
    outputTimestamps: new Map(),
  })
}

describe('P1.A — pinned-Chat spinner binds to its OWN project', () => {
  beforeEach(() => {
    touchInteraction.mockClear()
    activeProjectId = null
    reset()
  })

  it("recordTitleActivity attributes a pinned-Chat 'working' signal to its own project, even when a DIFFERENT workspace is active", () => {
    const ownProjectId = 'project-own'
    const paneId = agentChatId(ownProjectId, 'manager') // agent-chat:project-own
    // The user is looking at a DIFFERENT workspace right now.
    activeProjectId = 'project-other'

    const store = useActiveAgentsStore.getState()
    store.recordTitleActivity(paneId, true)

    // The spinner must light on the pinned chat's OWN project (regression:
    // pre-fix this returned 'idle' because the pane was bound to
    // 'project-other').
    expect(useActiveAgentsStore.getState().getProjectStatus(ownProjectId)).toBe('working')
    expect(useActiveAgentsStore.getState().getProjectStatus('project-other')).toBe('idle')
    // touchInteraction is keyed on the OWN project, not the viewed one.
    expect(touchInteraction).toHaveBeenCalledWith(ownProjectId)
  })

  it('bindPaneProject pre-registers the mapping before any title signal and is idempotent / non-clobbering', () => {
    const ownProjectId = 'project-own'
    const paneId = agentChatId(ownProjectId, 'manager')
    activeProjectId = 'project-other'

    const store = useActiveAgentsStore.getState()
    store.bindPaneProject(paneId, ownProjectId)
    expect(useActiveAgentsStore.getState().paneProjectMap.get(paneId)).toBe(ownProjectId)

    // A later title signal must not move the binding.
    store.recordTitleActivity(paneId, true)
    expect(useActiveAgentsStore.getState().paneProjectMap.get(paneId)).toBe(ownProjectId)
    expect(useActiveAgentsStore.getState().getProjectStatus(ownProjectId)).toBe('working')
  })

  it('a non-agent-chat pane (plain Cmd+T terminal) still binds to the active project', () => {
    const paneId = 'tab-some-random-terminal-id'
    activeProjectId = 'project-foreground'

    const store = useActiveAgentsStore.getState()
    store.recordTitleActivity(paneId, true)

    expect(useActiveAgentsStore.getState().getProjectStatus('project-foreground')).toBe('working')
    expect(touchInteraction).toHaveBeenCalledWith('project-foreground')
  })
})
