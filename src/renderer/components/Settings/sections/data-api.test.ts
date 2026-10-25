import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: vi.fn(),
  daemonCliGet: vi.fn(),
}))

import { daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import {
  SAMPLE_DATABASES,
  SAMPLE_STATUS,
  clustersLine,
  dbMajorChip,
  dbTypeLabel,
  formatSqlListen,
  setSqlDbAgentAccess,
  abortLiveMove,
  liveLine,
  parseLiveStatus,
  prepareLiveMoves,
  switchLiveMove,
  sqlErrorInfo,
  sqlErrorMessage,
  type SqlDatabase,
} from './data-api'

const post = vi.mocked(daemonCliPost)

describe('data-api helpers', () => {
  it('dbTypeLabel is sql / documents-in-same-DB', () => {
    expect(dbTypeLabel({ type: 'sql', documents: true })).toBe('sql / documents-in-same-DB')
  })

  it('sample fixture never carries secrets', () => {
    const blob = JSON.stringify({ SAMPLE_STATUS, SAMPLE_DATABASES }).toLowerCase()
    expect(blob).not.toContain('password')
    expect(blob).not.toContain('dsn')
    expect(blob).not.toContain('dbsec_')
    expect(blob).not.toContain('superuser')
    expect(SAMPLE_STATUS.supported).toBe(false)
    expect(SAMPLE_STATUS.port).toBe(5432)
    expect(SAMPLE_STATUS.publishHint).toContain('k2 publish subdomain')
    expect(blob).not.toContain('static ip')
  })

  it('sample databases list owner + grants with manage/read/write', () => {
    const sales = SAMPLE_DATABASES.find((d) => d.name === 'ws_sales') as SqlDatabase
    expect(sales.owner.level).toBe('write')
    expect(sales.owner.canManage).toBe(true)
    expect(sales.grants[0]?.level).toBe('read')
    expect(sales.bindRole).toBeTruthy()
    expect(sales.dbAgentAccess).toBe('off')
  })

  it('Settings → Data does not list skin people', () => {
    const blob = JSON.stringify({ SAMPLE_STATUS, SAMPLE_DATABASES }).toLowerCase()
    expect(blob).not.toContain('skin user')
    expect(blob).not.toContain('k2skn')
    expect(blob).not.toContain('skin_')
    const sales = SAMPLE_DATABASES.find((d) => d.name === 'ws_sales') as SqlDatabase
    expect(sales.grants.every((g) => g.workspace && g.projectId)).toBe(true)
  })

  it('formatSqlListen does not double-append port', () => {
    expect(formatSqlListen('localhost', 5432)).toBe('localhost:5432')
    expect(formatSqlListen('localhost:15432', 15432)).toBe('localhost:15432')
    expect(formatSqlListen('localhost:15432', 5432)).toBe('localhost:15432')
    expect(formatSqlListen(null, 5432)).toBeNull()
    expect(formatSqlListen('localhost')).toBe('localhost')
  })

  it('majors side by side: per-database chip and the clusters line', () => {
    const status = {
      clusters: [
        { major: 16, name: 'main', port: 5432, unit: 'postgresql@16-main.service', role: 'k2', state: 'online' },
        { major: 17, name: 'main', port: 5433, unit: 'postgresql@17-main.service', role: 'k2', state: 'online' },
      ],
      databases: [
        { id: 'a', name: 'ws_a', major: 16 },
        { id: 'b', name: 'ws_b', major: 17 },
        { id: 'c', name: 'ws_c', major: null },
      ],
      upgradeAvailable: [{ id: 'a', name: 'ws_a', from: 16, to: [17], check: 'k2 db upgrade ws_a --check' }],
    }
    expect(dbMajorChip(status, 'a')).toBe('PG 16 · upgrade available')
    expect(dbMajorChip(status, 'b')).toBe('PG 17')
    expect(dbMajorChip(status, 'c')).toBeNull()
    expect(dbMajorChip(status, 'missing')).toBeNull()
    expect(dbMajorChip(null, 'a')).toBeNull()
    expect(clustersLine(status)).toBe('PG 16 :5432 · PG 17 :5433')
    expect(clustersLine({ clusters: [status.clusters[0]] })).toBeNull()
    expect(clustersLine(SAMPLE_STATUS)).toBeNull()
  })

  it('sqlErrorMessage prefers daemon hint', () => {
    const err = new Error(JSON.stringify({ ok: false, error: { code: 'forbidden', hint: 'ask your human' } }))
    expect(sqlErrorInfo(err)).toEqual({ code: 'forbidden', hint: 'ask your human' })
    expect(sqlErrorMessage(err)).toBe('ask your human')
  })
})

describe('setSqlDbAgentAccess', () => {
  beforeEach(() => {
    post.mockReset()
    post.mockResolvedValue({ ok: true })
  })

  it('posts workspace/set with write when the Agent-tab toggle is on', async () => {
    await setSqlDbAgentAccess('/ws/sales', true)
    expect(post).toHaveBeenCalledTimes(1)
    expect(post).toHaveBeenCalledWith(primaryScope(), 'workspace/set', {
      project: '/ws/sales',
      fields: { db_agent_access: 'write' },
    })
  })

  it('posts workspace/set with off when the Agent-tab toggle is off', async () => {
    await setSqlDbAgentAccess('/ws/sales', false)
    expect(post).toHaveBeenCalledWith(primaryScope(), 'workspace/set', {
      project: '/ws/sales',
      fields: { db_agent_access: 'off' },
    })
  })
})

describe('live moves (PA40)', () => {
  beforeEach(() => {
    post.mockReset()
    post.mockResolvedValue({ ok: true })
  })

  it('reads the runs and the clusters that are not logical yet', () => {
    const v = parseLiveStatus({
      live: [{ db: 'ws_jobs', from: 16, to: 17, state: 'streaming', tables: { ready: 3, total: 3 }, lagBytes: 2 * 1024 ** 2, walStatus: 'reserved', lastError: null }],
      walLevels: [{ major: 16, walLevel: 'replica' }, { major: 17, walLevel: 'logical' }, { major: 18, walLevel: null }],
      liveHelper: true,
    })
    expect(v.runs).toEqual([{ db: 'ws_jobs', from: 16, to: 17, state: 'streaming', ready: 3, total: 3, lagBytes: 2 * 1024 ** 2, walStatus: 'reserved', lastError: null }])
    expect(v.notLogical).toEqual([16])
    expect(v.helper).toBe(true)
    expect(liveLine(v.runs[0])).toBe('3/3 tables · 2 MiB behind · WAL reserved')
    expect(parseLiveStatus(null)).toEqual({ runs: [], notLogical: [], helper: false })
  })

  it('posts switch, abort and prepare to the one upgrade route', async () => {
    await switchLiveMove('ws_jobs')
    await abortLiveMove('ws_jobs')
    await prepareLiveMoves(16)
    expect(post.mock.calls).toEqual([
      [primaryScope(), 'db/upgrade', { db: 'ws_jobs', switch: true }],
      [primaryScope(), 'db/upgrade', { db: 'ws_jobs', abort: true }],
      [primaryScope(), 'db/upgrade', { prepareLive: 16 }],
    ])
  })
})
