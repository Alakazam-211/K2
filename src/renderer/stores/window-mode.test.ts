// Presence S5 — window-mode store coverage: the whoami-derived default
// (owner → claimer, everyone else → viewer, null → stay unresolved),
// manual-toggle precedence over a slow default, the viewer-suppression
// predicate (unresolved never suppresses), and the daemon-ACK `capable`
// mirror.
//
// The whoami resolution is the PresenceKickButton cached fetch — mocked
// here at that boundary (per-test role), so no daemon-cli plumbing is
// exercised. The connect-host seam is mocked minimally (the store only
// registers a listener at import).

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the store import) ───────────────────

const whoami = vi.hoisted(() => ({
  role: null as string | null,
}))
vi.mock('@/components/Presence/PresenceKickButton', () => ({
  fetchViewerRole: vi.fn(async () => whoami.role),
}))

const host = vi.hoisted(() => ({
  listeners: [] as Array<() => void>,
}))
vi.mock('@/stores/connect-host', () => ({
  onActiveHostChange: vi.fn((fn: () => void) => {
    host.listeners.push(fn)
    return () => void (host.listeners = host.listeners.filter((f) => f !== fn))
  }),
}))

import { useToastStore } from './toast'
import {
  useWindowModeStore,
  deriveDefaultMode,
  isViewerModeActive,
  initWindowModeDefault,
  noteViewerInteractionBlocked,
  VIEWER_BLOCKED_TOAST,
  __resetWindowModeForTests,
} from './window-mode'

const flush = async (): Promise<void> => {
  await Promise.resolve()
  await Promise.resolve()
}

beforeEach(() => {
  __resetWindowModeForTests()
  useToastStore.setState({ toasts: [] })
  whoami.role = null
})

describe('deriveDefaultMode', () => {
  it('owner → claimer', () => {
    expect(deriveDefaultMode('owner')).toBe('claimer')
  })

  it('every non-owner role → viewer', () => {
    expect(deriveDefaultMode('admin')).toBe('viewer')
    expect(deriveDefaultMode('member')).toBe('viewer')
    expect(deriveDefaultMode('viewer')).toBe('viewer')
  })

  it('unresolved identity → null (no default applied)', () => {
    expect(deriveDefaultMode(null)).toBeNull()
  })
})

describe('initWindowModeDefault', () => {
  it('resolves owner windows to claimer', async () => {
    whoami.role = 'owner'
    initWindowModeDefault()
    await flush()
    const s = useWindowModeStore.getState()
    expect(s.mode).toBe('claimer')
    expect(s.resolved).toBe(true)
  })

  it('resolves member windows to viewer', async () => {
    whoami.role = 'member'
    initWindowModeDefault()
    await flush()
    const s = useWindowModeStore.getState()
    expect(s.mode).toBe('viewer')
    expect(s.resolved).toBe(true)
  })

  it('whoami failure leaves the store unresolved (daemon defaults rule)', async () => {
    whoami.role = null
    initWindowModeDefault()
    await flush()
    expect(useWindowModeStore.getState().resolved).toBe(false)
  })

  it('a manual toggle that lands first is never clobbered by the default', async () => {
    whoami.role = 'owner'
    initWindowModeDefault()
    // User flips to viewer BEFORE whoami resolves (demo mode).
    useWindowModeStore.getState().setMode('viewer')
    await flush()
    expect(useWindowModeStore.getState().mode).toBe('viewer')
    expect(useWindowModeStore.getState().resolved).toBe(true)
  })
})

describe('isViewerModeActive (client-side suppression predicate)', () => {
  it('unresolved never suppresses — even though mode reads viewer', () => {
    expect(useWindowModeStore.getState().mode).toBe('viewer')
    expect(isViewerModeActive()).toBe(false)
  })

  it('resolved viewer suppresses; resolved claimer does not', () => {
    useWindowModeStore.getState().setMode('viewer')
    expect(isViewerModeActive()).toBe(true)
    useWindowModeStore.getState().setMode('claimer')
    expect(isViewerModeActive()).toBe(false)
  })
})

describe('noteViewerInteractionBlocked', () => {
  it('unresolved does not toast', () => {
    expect(noteViewerInteractionBlocked()).toBe(false)
    expect(useToastStore.getState().toasts).toHaveLength(0)
  })

  it('resolved viewer toasts once then throttles', () => {
    useWindowModeStore.getState().setMode('viewer')
    expect(noteViewerInteractionBlocked()).toBe(true)
    const first = useToastStore.getState().toasts
    expect(first).toHaveLength(1)
    expect(first[0].message).toBe(VIEWER_BLOCKED_TOAST)
    expect(noteViewerInteractionBlocked()).toBe(true)
    expect(useToastStore.getState().toasts).toHaveLength(1)
  })

  it('resolved claimer does not toast', () => {
    useWindowModeStore.getState().setMode('claimer')
    expect(noteViewerInteractionBlocked()).toBe(false)
    expect(useToastStore.getState().toasts).toHaveLength(0)
  })
})

describe('capable mirror + host switch', () => {
  it('setCapable mirrors the daemon ACK', () => {
    expect(useWindowModeStore.getState().capable).toBe(true)
    useWindowModeStore.getState().setCapable(false)
    expect(useWindowModeStore.getState().capable).toBe(false)
  })

  it('a host switch resets and re-derives against the new host', async () => {
    whoami.role = 'member'
    initWindowModeDefault()
    await flush()
    useWindowModeStore.getState().setCapable(false)
    expect(useWindowModeStore.getState().mode).toBe('viewer')

    // Switch to a host where we are the owner.
    whoami.role = 'owner'
    for (const fn of host.listeners) fn()
    await flush()
    const s = useWindowModeStore.getState()
    expect(s.mode).toBe('claimer')
    expect(s.resolved).toBe(true)
    expect(s.capable).toBe(true)
  })
})
