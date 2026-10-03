import { describe, expect, it } from 'vitest'
import { parseUsageReport, type UsageReport } from './usageReport'

const ZERO = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, turns: 0 }

const EMPTY: UsageReport = {
  scope: '',
  workspace: null,
  total: ZERO,
  harnesses: [],
  models: [],
  workspaces: [],
  outside: ZERO,
  days: null,
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

  it('keeps a well-formed report as it is', () => {
    const t = { input_tokens: 10, output_tokens: 5, cache_read_tokens: 2, cache_write_tokens: 1, turns: 3 }
    const report: UsageReport = {
      scope: 'machine',
      workspace: null,
      total: t,
      harnesses: [{ ...t, harness: 'claude' }],
      models: [{ ...t, harness: 'claude', model: 'opus' }],
      workspaces: [{ ...t, path: '/w', outside: false }],
      outside: ZERO,
      days: [{ day: '2026-10-03', input_tokens: 10, output_tokens: 5, cache_read_tokens: 2 }],
    }
    expect(parseUsageReport(JSON.parse(JSON.stringify(report)))).toEqual(report)
  })

  it('drops rows without their key and zeroes missing counts', () => {
    const got = parseUsageReport({
      harnesses: [{ harness: 'codex', turns: 'x' }, { turns: 4 }, null],
      models: [{ harness: 'claude' }],
      workspaces: [{ path: '/a', outside: 'yes' }],
    })
    expect(got.harnesses).toEqual([{ ...ZERO, harness: 'codex' }])
    expect(got.models).toEqual([])
    expect(got.workspaces).toEqual([{ ...ZERO, path: '/a', outside: false }])
  })
})
