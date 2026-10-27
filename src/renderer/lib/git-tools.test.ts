import { describe, expect, it } from 'vitest'
import { asGitToolsStatus } from './git-tools'

describe('asGitToolsStatus', () => {
  it('refuses an answer with no state (an older server or a stub), so nothing is shown', () => {
    expect(() => asGitToolsStatus({})).toThrow('not a git-tools status')
    expect(() => asGitToolsStatus(null)).toThrow('not a git-tools status')
  })

  it('fills missing lists with empty ones', () => {
    const s = asGitToolsStatus({ state: 'ready', os: 'macos' })
    expect(s.state).toBe('ready')
    expect(s.waiting).toEqual([])
    expect(s.howTo).toEqual([])
  })
})
