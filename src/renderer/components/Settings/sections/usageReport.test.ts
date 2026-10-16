import { describe, expect, it } from 'vitest'
import { parseUsageReport, type TokenTotals, type UsageReport } from './usageReport'

const ZERO: TokenTotals = {
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: 0,
  cache_write_tokens: 0,
  reasoning_tokens: 0,
  requests: 0,
  turns: 0,
}

const EMPTY: UsageReport = {
  scope: '',
  workspace: null,
  tz: null,
  since_ms: null,
  until_ms: null,
  total: ZERO,
  harnesses: [],
  models: [],
  workspaces: [],
  logins: [],
  outside: ZERO,
  days: null,
  groups: null,
  prices: null,
}

describe('parseUsageReport', () => {
  it('turns anything that is not a report into an empty report', () => {
    for (const raw of [
      undefined,
      null,
      'not json',
      [],
      {},
      { error: 'role_required', required: 'owner', role: 'member' },
      { harnesses: {}, models: {}, workspaces: {} },
      { keepAwake: { mode: 'off' } },
    ]) {
      expect(parseUsageReport(raw)).toEqual(EMPTY)
    }
  })

  it('keeps a well-formed v2 report as it is (requests, cost, logins, groups, prices)', () => {
    const cost = { usd: 1.25, source: 'mixed', priced_requests: 3, provider_requests: 1, unpriced_requests: 1 }
    const t: TokenTotals = {
      input_tokens: 10,
      output_tokens: 5,
      cache_read_tokens: 2,
      cache_write_tokens: 1,
      reasoning_tokens: 2,
      requests: 4,
      turns: 4,
      cost,
    }
    const report: UsageReport = {
      scope: 'machine',
      workspace: null,
      tz: 'America/Denver',
      since_ms: 1791000000000,
      until_ms: null,
      total: t,
      harnesses: [{ ...t, harness: 'claude' }],
      models: [{ ...t, harness: 'claude', model: 'opus' }],
      workspaces: [{ ...t, path: '/w', outside: false }],
      logins: [{ ...t, account_id: 'acc_x', tool: 'claude', label: 'work' }],
      outside: ZERO,
      days: [{ day: '2026-10-03', input_tokens: 10, output_tokens: 5, cache_read_tokens: 2 }],
      groups: [{ ...t, key: 'claude/opus', label: 'opus' }],
      prices: { source: 'bundled', fetched_at: null, models: 186 },
    }
    expect(parseUsageReport(JSON.parse(JSON.stringify(report)))).toEqual(report)
  })

  it('reads a v1 report: turns become requests, no cost, no logins', () => {
    const got = parseUsageReport({
      scope: 'machine',
      total: { input_tokens: 1, output_tokens: 2, cache_read_tokens: 3, cache_write_tokens: 4, turns: 7 },
    })
    expect(got.total).toEqual({ ...ZERO, input_tokens: 1, output_tokens: 2, cache_read_tokens: 3, cache_write_tokens: 4, requests: 7, turns: 7 })
    expect('cost' in got.total).toBe(false)
    expect(got.logins).toEqual([])
    expect(got.groups).toBeNull()
  })

  it('drops rows without their key and zeroes missing counts', () => {
    const got = parseUsageReport({
      harnesses: [{ harness: 'codex', turns: 'x' }, { turns: 4 }, null],
      models: [{ harness: 'claude' }],
      workspaces: [{ path: '/a', outside: 'yes' }],
      logins: [{ tool: 'claude' }, { account_id: '', requests: 2 }],
      groups: [{ label: 'no key' }, { key: '' }],
    })
    expect(got.harnesses).toEqual([{ ...ZERO, harness: 'codex' }])
    expect(got.models).toEqual([])
    expect(got.workspaces).toEqual([{ ...ZERO, path: '/a', outside: false }])
    expect(got.logins).toEqual([{ ...ZERO, requests: 2, turns: 2, account_id: '', tool: '', label: '' }])
    expect(got.groups).toEqual([{ ...ZERO, key: '', label: '' }])
  })
})
