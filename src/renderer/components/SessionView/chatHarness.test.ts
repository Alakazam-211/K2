import { describe, expect, it } from 'vitest'
import { chatHarnessLabel, chatHarnessName, commandBasename } from './chatHarness'

describe('chat harness eligibility', () => {
  it('accepts path-qualified claude, codex, grok, and gemini', () => {
    expect(commandBasename('/usr/local/bin/claude')).toBe('claude')
    expect(commandBasename('C:\\Tools\\gemini.exe')).toBe('gemini.exe')
    expect(chatHarnessName({ command: '/opt/homebrew/bin/codex' })).toBe('codex')
    expect(chatHarnessName({ command: 'grok --resume abc' })).toBe('grok')
    expect(chatHarnessName({ command: '/bin/gemini' })).toBe('gemini')
  })

  it('uses commandHint when command was dropped on restore', () => {
    expect(chatHarnessName({ commandHint: '/usr/bin/claude' })).toBe('claude')
    expect(chatHarnessName({ command: undefined, commandHint: 'grok' })).toBe('grok')
  })

  it('uses ensure provider when the pane has no command', () => {
    expect(chatHarnessName({ provider: 'codex' })).toBe('codex')
    expect(chatHarnessName({ provider: 'gemini', command: undefined })).toBe('gemini')
  })

  it('does not select pi, cursor-agent, hermes, or a null program', () => {
    expect(chatHarnessName({ command: 'pi' })).toBeNull()
    expect(chatHarnessName({ command: '/bin/cursor-agent' })).toBeNull()
    expect(chatHarnessName({ command: 'hermes' })).toBeNull()
    expect(chatHarnessName({ provider: 'cursor' })).toBeNull()
    expect(chatHarnessName({ command: null, commandHint: null, provider: null })).toBeNull()
    expect(chatHarnessName({})).toBeNull()
  })

  it('labels the four harnesses and nothing else', () => {
    expect(chatHarnessLabel('claude')).toBe('Claude')
    expect(chatHarnessLabel('codex')).toBe('Codex')
    expect(chatHarnessLabel('grok')).toBe('Grok')
    expect(chatHarnessLabel('gemini')).toBe('Gemini')
    expect(chatHarnessLabel('pi')).toBe('Agent')
    expect(chatHarnessLabel(null)).toBe('Agent')
  })

  it('does not let a hint override a known non-v1 provider', () => {
    expect(chatHarnessName({ provider: 'pi', commandHint: 'claude' })).toBeNull()
  })
})
