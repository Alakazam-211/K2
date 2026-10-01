// Answer Q4 — the assistant acts only on rooms on the window's server; in
// a room on any other server it refuses actions and says why.

import { describe, it, expect, beforeEach, vi } from 'vitest'

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { assistantActionRefusal, assistantMayActOnServer } from './assistant-room-rule'
import { assistantRefusalFor, routeWorkspaceOp } from './workspace-ops-router'
import { roomHidden, roomShown, useWindowRoomStore } from '@/stores/window-room'
import { useToastStore } from '@/stores/toast'
import { useConnectHostStore } from '@/stores/connect-host'
import { testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'

const LOCAL = { hostKey: 'local', label: 'This computer' }
const DTL = { hostKey: 'dtl.k2.dev', label: 'dtl' }

describe('assistantActionRefusal (pure)', () => {
  it('a room on the window’s server: may act, no refusal', () => {
    expect(assistantMayActOnServer(LOCAL, LOCAL)).toBe(true)
    expect(assistantActionRefusal(LOCAL, LOCAL)).toBeNull()
    expect(assistantActionRefusal(DTL, DTL)).toBeNull()
  })

  it('a room on another server: refuses, names both servers, says it still chats', () => {
    expect(assistantMayActOnServer(DTL, LOCAL)).toBe(false)
    expect(assistantActionRefusal(DTL, LOCAL)).toBe(
      'This agent is on dtl. The assistant only changes tabs and terminals for agents on This computer, ' +
        'the server this window is connected to. Switch this window to dtl to let it act here. You can still chat with it.',
    )
    // The rule follows the switcher: on dtl, a This-computer room is refused.
    expect(assistantActionRefusal(LOCAL, DTL)).toContain('This agent is on This computer.')
  })
})

describe('the workspace:* router uses it (Q4)', () => {
  beforeEach(() => {
    useWindowRoomStore.setState({ shown: [], focused: null })
    useToastStore.setState({ toasts: [] })
    useConnectHostStore.setState({ activeHost: 'local' })
  })

  it('a focused room on another server: no op reaches its tabs, and the toast explains', () => {
    const ops: string[] = []
    const tabs = { room: { applyWorkspaceOp: (op: { kind: string }) => ops.push(op.kind) } }
    const remote = testRoom({ key: 'dtl.k2.dev|p:w', tabs, scope: fakeScope('dtl.k2.dev'), isPrimary: false })
    roomShown(remote)
    expect(assistantRefusalFor(remote)).toContain('This agent is on dtl.k2.dev.')
    routeWorkspaceOp({ kind: 'new-tab', payload: { cwd: '/x' } } as Parameters<typeof routeWorkspaceOp>[0])
    expect(ops).toEqual([])
    const toasts = useToastStore.getState().toasts
    expect(toasts).toHaveLength(1)
    expect(toasts[0].message).toContain('The assistant only changes tabs and terminals for agents on This computer')
    roomHidden(remote)
  })

  it('a focused room on the window’s server: the op goes through', () => {
    const ops: string[] = []
    const tabs = { room: { applyWorkspaceOp: (op: { kind: string }) => ops.push(op.kind) } }
    const local = testRoom({ key: 'local|p:w', tabs, scope: fakeScope('local', { remote: false }), isPrimary: false })
    roomShown(local)
    expect(assistantRefusalFor(local)).toBeNull()
    routeWorkspaceOp({ kind: 'new-tab', payload: { cwd: '/x' } } as Parameters<typeof routeWorkspaceOp>[0])
    expect(ops).toEqual(['new-tab'])
    expect(useToastStore.getState().toasts).toEqual([])
    roomHidden(local)
  })
})
