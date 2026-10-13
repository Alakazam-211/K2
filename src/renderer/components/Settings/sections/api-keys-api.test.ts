import { describe, it, expect } from 'vitest'
import {
  capsSummary,
  HIRE_FORM_DEFAULTS,
  hireLimitsFromForm,
  hireSummary,
  keyGrantsWorkspace,
  keyState,
  parseWorkspaceGrant,
  formatGrant,
  workspaceGrantSlug,
} from './api-keys-api'
import type { ApiKeyRow } from './api-keys-api'

function row(over: Partial<ApiKeyRow> = {}): ApiKeyRow {
  return {
    id: 'k1',
    label: 'test',
    scope: 'owner',
    createdAt: 1,
    revokedAt: null,
    disabledAt: null,
    keySet: true,
    anthropicKeySet: false,
    allowedWorkspaces: null,
    provider: null,
    baseUrl: null,
    capabilities: { hostSessions: true },
    ...over,
  }
}

describe('parseWorkspaceGrant / keyGrantsWorkspace', () => {
  it('treats null and empty as no grant', () => {
    expect(parseWorkspaceGrant(null).kind).toBe('none')
    expect(keyGrantsWorkspace(row({ allowedWorkspaces: null }), 'sales')).toBe(false)
  })

  it('star grants every slug', () => {
    expect(parseWorkspaceGrant('*').kind).toBe('all')
    expect(keyGrantsWorkspace(row({ allowedWorkspaces: '*' }), 'sales')).toBe(true)
    expect(keyGrantsWorkspace(row({ allowedWorkspaces: '*' }), 'Julie')).toBe(true)
  })

  it('JSON list matches case-insensitively', () => {
    const k = row({ allowedWorkspaces: JSON.stringify(['sales', 'docs']) })
    expect(keyGrantsWorkspace(k, 'sales')).toBe(true)
    expect(keyGrantsWorkspace(k, 'Sales')).toBe(true)
    expect(keyGrantsWorkspace(k, 'Julie')).toBe(false)
  })

  it('keyState prefers revoked over disabled', () => {
    expect(keyState(row({ disabledAt: 1 }))).toBe('disabled')
    expect(keyState(row({ revokedAt: 1, disabledAt: 1 }))).toBe('revoked')
    expect(keyState(row())).toBe('active')
  })

  it('workspaceGrantSlug prefers name then basename', () => {
    expect(workspaceGrantSlug({ name: 'Sales', path: '/x/y' })).toBe('Sales')
    expect(workspaceGrantSlug({ name: '', path: '/home/k2/AI Projects/sales' })).toBe('sales')
  })

  it('formatGrant is human readable', () => {
    expect(formatGrant('*')).toContain('*')
    expect(formatGrant(JSON.stringify(['a', 'b']))).toBe('a, b')
    expect(formatGrant(null)).toContain('none')
  })
})

describe('hire scope helpers (0.45.6)', () => {
  it('capsSummary names the db and hire doors', () => {
    expect(capsSummary({ hostSessions: false, db: true, hire: true })).toBe('db, hire')
  })

  it('hireLimitsFromForm validates numbers and the db ceiling', () => {
    expect(hireLimitsFromForm(HIRE_FORM_DEFAULTS, false)).toEqual({
      maxMembersPerDay: 20,
      maxLiveMembers: 100,
      maxMembersPerTeam: 12,
      db: 'off',
      link: false,
      retire: false,
      reach: 'none',
    })
    expect(() => hireLimitsFromForm({ ...HIRE_FORM_DEFAULTS, maxLiveMembers: 'x' }, false)).toThrow(/Live agents/)
    expect(() => hireLimitsFromForm({ ...HIRE_FORM_DEFAULTS, db: 'read' }, false)).toThrow(/db door/)
    expect(hireLimitsFromForm({ ...HIRE_FORM_DEFAULTS, db: 'read', folder: ' acme ' }, true)).toMatchObject({
      db: 'read',
      folder: 'acme',
    })
  })

  it('hireSummary is empty without limits', () => {
    expect(hireSummary(null)).toBe('')
  })
})
