import { describe, it, expect } from 'vitest'
import type { ComputeNode } from '../sections/compute-api'
import type { ComputeMachine } from './compute-account'
import { controllerSub, mergeMachines, nodeNameFor, REQUEST_TTL_MS, sasAgrees } from './compute-machines'

const NOW = Date.parse('2026-10-11T12:00:00Z')

function node(over: Partial<ComputeNode> = {}): ComputeNode {
  return {
    id: 'n1',
    name: 'mini-1',
    fingerprint: 'AA'.repeat(32),
    os: 'macos',
    arch: 'aarch64',
    labels: {},
    state: 'active',
    enrolledVia: 'code',
    enrolledAt: 1,
    confirmedBy: 'owner',
    controllerPause: null,
    lastSeen: 1,
    lastRoute: null,
    protocol: 1,
    nodeVersion: null,
    routes: [],
    online: false,
    liveRoute: null,
    queued: 0,
    running: 0,
    ...over,
  }
}

function machine(over: Partial<ComputeMachine> = {}): ComputeMachine {
  return {
    sub: 'z13',
    node_fp: 'bb'.repeat(32),
    node_name: 'z13',
    node_state: 'unenrolled',
    controller: null,
    control: 'active',
    sas: null,
    os: 'linux',
    arch: 'x86_64',
    cpu_cores: 16,
    mem_bytes: 1,
    disk_total_bytes: 2,
    disk_free_bytes: 3,
    node_version: null,
    k2_version: null,
    updated_at: '2026-10-11T11:59:00Z',
    ...over,
  }
}

describe('mergeMachines', () => {
  it('matches account machines to nodes by key fingerprint (case-insensitive)', () => {
    const rows = mergeMachines([node()], [machine({ node_fp: 'aa'.repeat(32), sub: 'mini' })], 'rosson.k2.dev', {}, NOW)
    expect(rows).toHaveLength(1)
    expect(rows[0]).toMatchObject({ name: 'mini-1', sub: 'mini', status: 'on', cpuCores: 16, os: 'macos' })
  })

  it('code-enrolled nodes show without an account row; revoked ones do not', () => {
    const rows = mergeMachines([node(), node({ id: 'n2', name: 'gone', state: 'revoked' })], [], 'rosson.k2.dev', {}, NOW)
    expect(rows.map((r) => r.name)).toEqual(['mini-1'])
    expect(rows[0].sub).toBeNull()
  })

  it('node facts win over the account row (offer caps and free disk)', () => {
    const n = node({
      online: true,
      offer: {
        control: 'active',
        available: true,
        unavailable: [],
        caps: { cpu_millis: 6000, mem_bytes: 8, disk_budget_bytes: 9, hard: true },
        free: { cpu_millis: 1, mem_bytes: 1, disk_free_bytes: 77 },
        parallelMax: 1,
        tools: {},
      },
    })
    const [row] = mergeMachines([n], [machine({ node_fp: 'aa'.repeat(32) })], 'rosson.k2.dev', {}, NOW)
    expect(row).toMatchObject({ cpuCores: 6, memBytes: 8, diskFreeBytes: 77, online: true })
  })

  it('statuses: pending, retiring, off, requested, elsewhere', () => {
    const rows = mergeMachines(
      [node({ state: 'pending' }), node({ id: 'n2', name: 'old', fingerprint: 'cc'.repeat(32), retiring: true })],
      [
        machine({ sub: 'free1', node_fp: 'd1'.repeat(32), node_name: 'free1' }),
        machine({ sub: 'asked', node_fp: 'd2'.repeat(32), node_name: 'asked' }),
        machine({ sub: 'other', node_fp: 'd3'.repeat(32), node_name: 'other', node_state: 'online', controller: 'else.k2.dev' }),
        machine({ sub: 'stale', node_fp: 'd4'.repeat(32), node_name: 'stale', node_state: 'offline', controller: 'rosson.k2.dev' }),
        machine({ sub: 'late', node_fp: 'd5'.repeat(32), node_name: 'late' }),
      ],
      'rosson.k2.dev',
      { asked: NOW - 1000, late: NOW - REQUEST_TTL_MS - 1 },
      NOW,
    )
    const by = Object.fromEntries(rows.map((r) => [r.name, r]))
    expect(by['mini-1'].status).toBe('pending')
    expect(by.old.status).toBe('retiring')
    expect(by.free1.status).toBe('off')
    expect(by.asked.status).toBe('requested')
    expect(by.other.status).toBe('elsewhere')
    expect(by.other.elsewhere).toBe('else.k2.dev')
    expect(by.stale.status).toBe('off')
    expect(by.late.status).toBe('off')
    expect(by.free1.online).toBe(true)
  })
})

describe('helpers', () => {
  it('controllerSub only for k2.dev names', () => {
    expect(controllerSub('rosson.k2.dev')).toBe('rosson')
    expect(controllerSub('https://Rosson.k2.dev/')).toBe('rosson')
    expect(controllerSub('192.168.1.5')).toBeNull()
    expect(controllerSub('a.b.k2.dev')).toBeNull()
    expect(controllerSub('')).toBeNull()
  })

  it('node names follow the daemon grammar', () => {
    expect(nodeNameFor({ node_name: 'Mini_1', sub: 'x' })).toBe('mini-1')
    expect(nodeNameFor({ node_name: null, sub: 'z13' })).toBe('z13')
    expect(nodeNameFor({ node_name: '--', sub: '' })).toBe('node')
    expect(nodeNameFor({ node_name: 'a'.repeat(40), sub: '' })).toHaveLength(32)
  })

  it('codes agree only when both are the same six digits', () => {
    expect(sasAgrees(node({ sas: '123 456' }), machine({ sas: '123456' }))).toBe(true)
    expect(sasAgrees(node({ sas: '123 456' }), machine({ sas: '654321' }))).toBe(false)
    expect(sasAgrees(node({ sas: null }), machine({ sas: null }))).toBe(false)
    expect(sasAgrees(null, machine({ sas: '123456' }))).toBe(false)
  })
})
