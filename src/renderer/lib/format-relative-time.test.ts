// Moved verbatim from components/AgentOps/ops-api.test.ts when the Agent
// Ops fleet view was deleted in 0.40.31 (formatRelativeTime survives as a
// shared helper for the Feedback surfaces + ProjectChatPanel).

import { describe, it, expect } from 'vitest'
import { formatRelativeTime } from './format-relative-time'

describe('formatRelativeTime', () => {
  const now = 1_000_000
  it('renders coarse human buckets', () => {
    expect(formatRelativeTime(now, now)).toBe('just now')
    expect(formatRelativeTime(now - 10, now)).toBe('just now')
    expect(formatRelativeTime(now - 60, now)).toBe('1m ago')
    expect(formatRelativeTime(now - 120, now)).toBe('2m ago')
    expect(formatRelativeTime(now - 3600, now)).toBe('1h ago')
    expect(formatRelativeTime(now - 7200, now)).toBe('2h ago')
    expect(formatRelativeTime(now - 86400, now)).toBe('1d ago')
  })
  it('renders "—" for a null/absent timestamp', () => {
    expect(formatRelativeTime(null, now)).toBe('—')
    expect(formatRelativeTime(undefined, now)).toBe('—')
  })
  it('clamps a future timestamp (clock skew) to "just now"', () => {
    expect(formatRelativeTime(now + 500, now)).toBe('just now')
  })
})
