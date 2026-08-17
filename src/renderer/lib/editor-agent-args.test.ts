import { describe, it, expect } from 'vitest'
import { buildEditorAgentArgs, commandBaseName } from './editor-agent-args'

describe('buildEditorAgentArgs', () => {
  it('splits a path command to the basename', () => {
    expect(commandBaseName('/usr/local/bin/grok')).toBe('grok')
    expect(commandBaseName('claude')).toBe('claude')
  })

  it('uses --append-system-prompt for claude', () => {
    expect(
      buildEditorAgentArgs({
        command: 'claude',
        baseArgs: ['--dangerously-skip-permissions'],
        systemBrief: 'BRIEF',
        userMessage: 'DO IT',
      }),
    ).toEqual([
      '--dangerously-skip-permissions',
      '--append-system-prompt',
      'BRIEF',
      'DO IT',
    ])
  })

  it('forces grok fullscreen and a positional prompt', () => {
    expect(
      buildEditorAgentArgs({
        command: '/Users/z3thon/.grok/bin/grok',
        baseArgs: ['--always-approve', '--minimal'],
        systemBrief: 'BRIEF',
        userMessage: 'DO IT',
      }),
    ).toEqual(['--always-approve', '--fullscreen', 'BRIEF\n\nDO IT'])
  })
})
