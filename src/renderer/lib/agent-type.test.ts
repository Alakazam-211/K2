import { describe, expect, it } from 'vitest'
import { isBuiltinAgentType } from './agent-type'

describe('isBuiltinAgentType', () => {
  it('accepts k2 and legacy k2so', () => {
    expect(isBuiltinAgentType('k2')).toBe(true)
    expect(isBuiltinAgentType('k2so')).toBe(true)
  })

  it('rejects other agent types and empty values', () => {
    expect(isBuiltinAgentType('custom')).toBe(false)
    expect(isBuiltinAgentType('manager')).toBe(false)
    expect(isBuiltinAgentType('agent')).toBe(false)
    expect(isBuiltinAgentType('')).toBe(false)
    expect(isBuiltinAgentType(undefined)).toBe(false)
    expect(isBuiltinAgentType(null)).toBe(false)
  })
})
