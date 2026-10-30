import { describe, expect, it } from 'vitest'
import { confirmWays, thisMachine, thisMachineLower, touchIdOrPassword, touchIdOrVaultPassword } from './desktop-words'

describe('this computer and confirm words', () => {
  it('Windows: this PC, and a password (no Touch ID there)', () => {
    expect(thisMachine('windows')).toBe('This PC')
    expect(thisMachineLower('windows')).toBe('this PC')
    expect(touchIdOrPassword('windows')).toBe('your password')
    expect(confirmWays('windows')).toBe('your login or the vault password')
    expect(touchIdOrVaultPassword('windows')).toBe('your vault password')
  })

  it('macOS and Linux keep the words they had', () => {
    for (const os of ['mac', 'linux'] as const) {
      expect(thisMachine(os)).toBe('This Mac')
      expect(thisMachineLower(os)).toBe('this Mac')
      expect(touchIdOrPassword(os)).toBe('Touch ID or your password')
      expect(confirmWays(os)).toBe('Touch ID, your login or the vault password')
      expect(touchIdOrVaultPassword(os)).toBe('Touch ID or your vault password')
    }
  })
})
