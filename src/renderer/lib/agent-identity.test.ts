// Agent rename live update — the pure patch helpers behind every surface.

import { describe, it, expect } from 'vitest'
import {
  agentIdentityDisplayName,
  agentIdentityMatchesPath,
  agentIdentityNeedsRefetch,
  applyAgentIdentity,
} from './agent-identity'
import type { AgentIdentityChangedEvent } from '@/stores/session-events'

function ev(over: Partial<AgentIdentityChangedEvent> = {}): AgentIdentityChangedEvent {
  return {
    kind: 'agent_identity_changed',
    projectId: 'p1',
    workspacePath: '/w/sales',
    fields: ['displayName', 'name'],
    name: 'Revenue Desk',
    displayName: 'Revenue Desk',
    handle: 'sales',
    color: '#ff00aa',
    ...over,
  }
}

const rows = [
  { id: 'p1', name: 'Sales', color: '#ff00aa', handle: 'sales', path: '/w/sales' },
  { id: 'p2', name: 'Ops', color: '#000000', handle: 'ops', path: '/w/ops' },
]

describe('applyAgentIdentity', () => {
  it('patches only the matching row with the current values; others keep identity', () => {
    const next = applyAgentIdentity(rows, ev())
    expect(next).not.toBe(rows)
    expect(next[0]).toEqual({ ...rows[0], name: 'Revenue Desk' })
    expect(next[1]).toBe(rows[1])
  })

  it('returns the SAME array when nothing differs (no re-render)', () => {
    expect(applyAgentIdentity(rows, ev({ name: 'Sales' }))).toBe(rows)
    expect(applyAgentIdentity(rows, ev({ projectId: 'nobody' }))).toBe(rows)
  })

  it('follows a handle rename and a color change; empty values never blank a row', () => {
    const next = applyAgentIdentity(rows, ev({ fields: ['handle', 'color'], name: '', handle: 'revenue', color: '#00ff00' }))
    expect(next[0]).toEqual({ ...rows[0], handle: 'revenue', color: '#00ff00' })
  })
})

describe('agentIdentityNeedsRefetch', () => {
  it('patchable words need no refetch; image or an unknown word does', () => {
    expect(agentIdentityNeedsRefetch(ev())).toBe(false)
    expect(agentIdentityNeedsRefetch(ev({ fields: ['handle', 'color'] }))).toBe(false)
    expect(agentIdentityNeedsRefetch(ev({ fields: ['image'] }))).toBe(true)
    expect(agentIdentityNeedsRefetch(ev({ fields: ['name', 'pronouns'] }))).toBe(true)
  })
})

describe('agentIdentityMatchesPath / agentIdentityDisplayName', () => {
  it('matches the same folder, ignoring a trailing slash; never an empty path', () => {
    expect(agentIdentityMatchesPath(ev(), '/w/sales')).toBe(true)
    expect(agentIdentityMatchesPath(ev(), '/w/sales/')).toBe(true)
    expect(agentIdentityMatchesPath(ev(), '/w/sales-other')).toBe(false)
    expect(agentIdentityMatchesPath(ev(), '')).toBe(false)
    expect(agentIdentityMatchesPath(ev({ workspacePath: '' }), '/w/sales')).toBe(false)
  })

  it('prefers the AGENT.md display name, falls back to the sidebar name, else null', () => {
    expect(agentIdentityDisplayName(ev({ displayName: 'Desk', name: 'Row' }))).toBe('Desk')
    expect(agentIdentityDisplayName(ev({ displayName: ' ', name: 'Row' }))).toBe('Row')
    expect(agentIdentityDisplayName(ev({ displayName: '', name: '' }))).toBe(null)
  })
})
