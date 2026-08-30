import { describe, it, expect } from 'vitest'
import {
  SAMPLE_DATABASES,
  SAMPLE_STATUS,
  dbTypeLabel,
  formatSqlListen,
  sqlErrorInfo,
  sqlErrorMessage,
  type SqlDatabase,
} from './data-api'

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

  it('sqlErrorMessage prefers daemon hint', () => {
    const err = new Error(JSON.stringify({ ok: false, error: { code: 'forbidden', hint: 'ask your human' } }))
    expect(sqlErrorInfo(err)).toEqual({ code: 'forbidden', hint: 'ask your human' })
    expect(sqlErrorMessage(err)).toBe('ask your human')
  })
})
