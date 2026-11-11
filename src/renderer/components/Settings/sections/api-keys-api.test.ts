import { describe, it, expect } from 'vitest'
import {
  capsSummary,
  ceilingFormFromLimits,
  ceilingFromForm,
  ceilingSummary,
  EMPTY_CEILING_FORM,
  parseCeilingOptions,
  HIRE_FORM_DEFAULTS,
  hireLimitsFromForm,
  hireSummary,
  keyGrantsWorkspace,
  keyState,
  parseWorkspaceGrant,
  formatGrant,
  workspaceGrantSlug,
  grantWords,
  hireWords,
  joinWords,
  keyDates,
  keyName,
  reachWords,
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

  it('hireSummary leaves the App access limit to its own line', () => {
    expect(
      hireSummary({
        maxMembersPerDay: 1,
        maxLiveMembers: 2,
        maxMembersPerTeam: 3,
        db: 'off',
        link: false,
        linkTargets: 'created',
        retire: false,
        reach: 'none',
        folder: '',
        apps: [{ app: 'svc', rooms: ['self'], roles: [] }],
      }),
    ).toBe('1/day · 2 live · 3 per team · db off')
  })
})

describe('App access limit helpers (0.45.6)', () => {
  const opts = parseCeilingOptions({
    apps: [
      { id: 'svc-1', name: 'jobs', roles: [{ id: 'r-m', name: 'Member', appId: 'svc-1', rooms: ['a'] }], roomIds: ['a'] },
      { name: 'no id is dropped' },
    ],
    rooms: [{ id: 'self', handle: 'self' }, { id: 'a', handle: 'job-a', name: 'Job A' }, 7],
  })

  it('parses options and drops rows without ids', () => {
    expect(opts.apps.map((a) => a.id)).toEqual(['svc-1'])
    expect(opts.rooms.map((r) => r.id)).toEqual(['self', 'a'])
    expect(parseCeilingOptions('garbage')).toEqual({ apps: [], rooms: [] })
  })

  it('summarizes each App in one line, and the empty limit as No App access', () => {
    expect(ceilingSummary([], opts)).toBe('No App access')
    expect(ceilingSummary(undefined, opts)).toBe('No App access')
    expect(ceilingSummary([{ app: 'svc-1', rooms: ['self'], roles: [] }], opts)).toBe(
      'jobs · its own room · no role (Thread only)',
    )
    expect(ceilingSummary([{ app: 'svc-1', rooms: ['self', 'a', 'b'], roles: ['r-m'] }], opts)).toBe(
      'jobs · 3 rooms · up to Member',
    )
    expect(ceilingSummary([{ app: 'gone', rooms: ['a'], roles: ['x'] }], opts)).toBe('gone · 1 room · up to x')
  })

  it('turns the form into the stored shape and refuses an App with no rooms', () => {
    expect(ceilingFromForm(EMPTY_CEILING_FORM, opts)).toEqual([])
    expect(ceilingFromForm({ app: 'svc-1', rooms: ['self', 'a'], role: 'r-m' }, opts)).toEqual([
      { app: 'svc-1', rooms: ['self', 'a'], roles: ['r-m'] },
    ])
    expect(ceilingFromForm({ app: 'svc-1', rooms: ['a'], role: '' }, opts)).toEqual([
      { app: 'svc-1', rooms: ['a'], roles: [] },
    ])
    expect(() => ceilingFromForm({ app: 'svc-1', rooms: [], role: '' }, opts)).toThrow(/Pick at least one room on jobs/)
    expect(ceilingFormFromLimits([{ app: 'svc-1', rooms: ['a'], roles: ['r-m'] }])).toEqual({
      app: 'svc-1',
      rooms: ['a'],
      role: 'r-m',
    })
    expect(ceilingFormFromLimits(undefined)).toEqual(EMPTY_CEILING_FORM)
  })
})

describe('plain words for Settings → API Keys', () => {
  it('joinWords lists like a sentence', () => {
    expect(joinWords([])).toBe('')
    expect(joinWords(['a'])).toBe('a')
    expect(joinWords(['a', 'b'])).toBe('a and b')
    expect(joinWords(['a', 'b', 'c'])).toBe('a, b and c')
  })

  it('grantWords says where a key reaches', () => {
    expect(grantWords('*')).toBe('in every workspace')
    expect(grantWords(null)).toBe('in no workspace yet')
    expect(grantWords(JSON.stringify(['notes', 'docs']))).toBe('in notes and docs')
    expect(grantWords(JSON.stringify(['a', 'b', 'c', 'd', 'e']))).toBe('in a, b and 3 more workspaces')
  })

  it('reachWords names what a key does without route or flag names', () => {
    expect(reachWords(row({ allowedWorkspaces: '*', capabilities: { hostSessions: true, db: true } }))).toBe(
      'Agent sessions and databases in every workspace.',
    )
    // Sandboxes alone don't need a workspace.
    expect(reachWords(row({ capabilities: { sandboxes: true } }))).toBe('Sandboxes.')
    expect(reachWords(row({ capabilities: { hire: true } }))).toBe('Hires agents.')
    expect(reachWords(row({ allowedWorkspaces: '["notes"]', capabilities: { canonicalMessage: true, hire: true } }))).toBe(
      'Messages in notes; hires agents.',
    )
    expect(reachWords(row({ capabilities: { hire: true } }), true)).toBe('')
    expect(reachWords(row({ capabilities: {} }))).toBe('Can’t reach anything yet.')
    expect(reachWords(row({ scope: 'status' }))).toMatch(/status.*Nothing else\.$/)
  })

  it('hireWords puts a hire key’s limits in a sentence', () => {
    expect(
      hireWords({ maxMembersPerDay: 5, maxLiveMembers: 50, maxMembersPerTeam: 4, db: 'read', link: true, linkTargets: 'created', retire: false, reach: 'message', folder: '' }),
    ).toBe('Hires up to 5 agents a day (50 at once, 4 per team). May message and link into teams the agents it hired. They can read its databases.')
    expect(hireWords(null)).toBe('')
  })

  it('keyDates says when a key was made, turned off or revoked', () => {
    const made = keyDates(row({ createdAt: 1790000000 }))
    expect(made).toMatch(/^Created .*2026$/)
    expect(keyDates(row({ createdAt: 1790000000, disabledAt: 1791000000 }))).toMatch(/· Turned off .*2026$/)
    // Revoked wins over turned off; milliseconds read the same as seconds.
    expect(keyDates(row({ createdAt: 1790000000 * 1000, disabledAt: 1, revokedAt: 1791000000 }))).toMatch(/^Created .*2026 · Revoked .*2026$/)
    expect(keyDates(row({ createdAt: 0, anthropicKeySet: true, provider: 'openai' }))).toBe('Uses its own OpenAI key')
  })

  it('keyName names an unlabelled key', () => {
    expect(keyName({ label: '  ' })).toBe('Unnamed key')
    expect(keyName({ label: 'Support chat' })).toBe('Support chat')
  })
})
