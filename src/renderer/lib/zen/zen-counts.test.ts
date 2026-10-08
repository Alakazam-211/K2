import { describe, expect, it } from 'vitest'
import { zenCountsText } from './zen-counts'

describe('zenCountsText', () => {
  it('joins the non-zero parts with plurals', () => {
    expect(zenCountsText({ subagents: 2, tools: 14, commands: 5 })).toBe('2 subagents · 14 tools · 5 commands')
    expect(zenCountsText({ subagents: 1, tools: 1, commands: 1 })).toBe('1 subagent · 1 tool · 1 command')
    expect(zenCountsText({ subagents: 0, tools: 3, commands: 0 })).toBe('3 tools')
    expect(zenCountsText({ subagents: 1, tools: 0, commands: 0 })).toBe('1 subagent')
  })

  it('is null with nothing to say', () => {
    expect(zenCountsText({ subagents: 0, tools: 0, commands: 0 })).toBeNull()
    expect(zenCountsText(null)).toBeNull()
    expect(zenCountsText(undefined)).toBeNull()
  })
})
