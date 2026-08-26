import { describe, it, expect } from 'vitest'
import {
  SAMPLE_DATABASES,
  SAMPLE_STATUS,
  dbTypeLabel,
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
  })

  it('sample databases list owner + grants with manage/read/write', () => {
    const sales = SAMPLE_DATABASES.find((d) => d.name === 'ws_sales') as SqlDatabase
    expect(sales.owner.level).toBe('write')
    expect(sales.owner.canManage).toBe(true)
    expect(sales.grants[0]?.level).toBe('read')
    expect(sales.bindRole).toBeTruthy()
  })

  it('sqlErrorMessage prefers daemon hint', () => {
    const err = new Error(JSON.stringify({ ok: false, error: { code: 'forbidden', hint: 'ask your human' } }))
    expect(sqlErrorInfo(err)).toEqual({ code: 'forbidden', hint: 'ask your human' })
    expect(sqlErrorMessage(err)).toBe('ask your human')
  })
})
