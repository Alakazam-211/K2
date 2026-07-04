// Slice 5 — grok TITLE-driven permission state (set + clear).
//
// Grok announces an open tool-permission gate in the terminal TITLE
// (`⚠ Action Required - ` prefix) and has NO lifecycle hooks, so the
// title is its ONLY permission source. `recordTitlePermission` must:
//   - set the SAME 'permission' pane state Claude's hook drives,
//   - CLEAR it when the prefix goes away (gate resolved) — something
//     `recordTitleActivity` refuses to do by contract,
//   - while never weakening Claude's hook semantics: a HOOK-set
//     permission must stay un-clearable from the title path.
//
// vitest env is node (no Tauri / daemon). Mock the load-time boundaries
// so importing the store is inert, then drive the store directly
// (pattern from active-agents-spinner.test.ts).

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

import {
  useActiveAgentsStore,
  __resetAgentStateForHostSwitch,
} from './active-agents'
import { useToastStore } from './toast'
import { agentChatId } from '@/lib/terminal-id'

function reset(): void {
  // Clears the store AND the module-level maps/sets (incl. the
  // title-permission ownership set).
  __resetAgentStateForHostSwitch()
}

describe('recordTitlePermission — grok gate set/clear', () => {
  beforeEach(() => {
    touchInteraction.mockClear()
    activeProjectId = null
    reset()
    useToastStore.setState({ toasts: [] })
  })

  it('sets the pane to permission (the same state the claude hook drives)', () => {
    const s = useActiveAgentsStore.getState()
    s.recordTitlePermission('tab-grok-1', true)
    expect(useActiveAgentsStore.getState().getPaneStatus('tab-grok-1')).toBe(
      'permission',
    )
  })

  it('surfaces the same "needs your permission" toast as the hook path, deduped', () => {
    const s = useActiveAgentsStore.getState()
    s.recordTitlePermission('tab-grok-1', true)
    // grok's title event re-fires while the gate stays open — no
    // duplicate toast (hook-path dedupe reused).
    s.recordTitlePermission('tab-grok-1', true)
    const toasts = useToastStore.getState().toasts
    expect(toasts.length).toBe(1)
    expect(toasts[0].message).toContain('permission')
  })

  it('clears a TITLE-owned permission when the ⚠ prefix goes away', () => {
    const s = useActiveAgentsStore.getState()
    s.recordTitlePermission('tab-grok-1', true)
    expect(useActiveAgentsStore.getState().getPaneStatus('tab-grok-1')).toBe(
      'permission',
    )
    // Gate resolved: grok's next title has no ⚠ prefix.
    s.recordTitlePermission('tab-grok-1', false)
    expect(useActiveAgentsStore.getState().getPaneStatus('tab-grok-1')).toBe(
      'idle',
    )
  })

  it('after a clear, the next working title re-arms working (full cycle)', () => {
    const s = useActiveAgentsStore.getState()
    s.recordTitleActivity('tab-grok-1', true) // ⠙ - Thinking - grok
    s.recordTitlePermission('tab-grok-1', true) // ⚠ Action Required - …
    s.recordTitlePermission('tab-grok-1', false) // gate resolved
    s.recordTitleActivity('tab-grok-1', true) // turn continues
    expect(useActiveAgentsStore.getState().getPaneStatus('tab-grok-1')).toBe(
      'working',
    )
  })

  it("recordTitleActivity still can NOT clear a permission state (existing contract)", () => {
    const s = useActiveAgentsStore.getState()
    s.recordTitlePermission('tab-grok-1', true)
    s.recordTitleActivity('tab-grok-1', true)
    s.recordTitleActivity('tab-grok-1', false)
    expect(useActiveAgentsStore.getState().getPaneStatus('tab-grok-1')).toBe(
      'permission',
    )
  })

  it('a HOOK-set permission is NOT clearable from the title path (claude unweakened)', () => {
    const s = useActiveAgentsStore.getState()
    // Claude's real lifecycle hook sets permission…
    s.handleLifecycleEvent('tab-claude-1', 'tab-1', 'permission')
    expect(useActiveAgentsStore.getState().getPaneStatus('tab-claude-1')).toBe(
      'permission',
    )
    // …and a stray non-⚠ title event (claude titles churn every ~1s)
    // must not clear it.
    s.recordTitlePermission('tab-claude-1', false)
    expect(useActiveAgentsStore.getState().getPaneStatus('tab-claude-1')).toBe(
      'permission',
    )
  })

  it('a hook event supersedes title ownership: the title can no longer clear', () => {
    const s = useActiveAgentsStore.getState()
    // Title sets it (title-owned)…
    s.recordTitlePermission('tab-x', true)
    // …then a REAL hook permission event arrives for the same pane.
    s.handleLifecycleEvent('tab-x', 'tab-1', 'permission')
    // The title clear must now no-op — the hook owns the state.
    s.recordTitlePermission('tab-x', false)
    expect(useActiveAgentsStore.getState().getPaneStatus('tab-x')).toBe(
      'permission',
    )
  })

  it('clear no-ops for panes that never had a title permission', () => {
    const s = useActiveAgentsStore.getState()
    s.recordTitleActivity('tab-y', true)
    s.recordTitlePermission('tab-y', false)
    expect(useActiveAgentsStore.getState().getPaneStatus('tab-y')).toBe(
      'working',
    )
  })

  it('binds an agent-chat pane to its OWN project (P1.A discipline)', () => {
    const ownProjectId = 'project-own'
    const paneId = agentChatId(ownProjectId, 'manager')
    activeProjectId = 'project-other' // user is looking elsewhere
    const s = useActiveAgentsStore.getState()
    s.recordTitlePermission(paneId, true)
    expect(
      useActiveAgentsStore.getState().getProjectStatus(ownProjectId),
    ).toBe('permission')
    expect(
      useActiveAgentsStore.getState().getProjectStatus('project-other'),
    ).toBe('idle')
  })
})
