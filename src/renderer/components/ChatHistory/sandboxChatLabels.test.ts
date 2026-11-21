import { describe, expect, it } from 'vitest'
import { sandboxChatLabels, type SandboxChat } from './sandboxChatLabels'

const row = (extra: Partial<SandboxChat>): SandboxChat => ({
  sessionId: 's1',
  title: 't',
  timestamp: 1,
  messageCount: 0,
  ...extra,
})

describe('sandbox chat labels (0.46.17)', () => {
  it('says Sandboxed only when the daemon reports every session sandboxed', () => {
    expect(sandboxChatLabels([row({ sandboxed: true }), row({ sandboxed: true })]).header).toBe('Sandboxed')
    expect(sandboxChatLabels([row({ sandboxed: true }), row({ sandboxed: false })]).header).toBe('Cell sessions')
    // An older daemon sends no field: never assumed sandboxed.
    expect(sandboxChatLabels([row({})]).header).toBe('Cell sessions')
    expect(sandboxChatLabels([]).header).toBe('Cell sessions')
  })

  it('a row says what it is', () => {
    const { line } = sandboxChatLabels([])
    expect(line(row({ sandboxed: false }))).toBe('Unsandboxed: runs as you')
    expect(line(row({}))).toBe('sandbox not confirmed by this server')
    expect(line(row({ sandboxed: true, reopenable: true }))).toBe('sandbox · click to re-launch')
    expect(line(row({ sandboxed: true, reopenable: false }))).toBe("sandbox · this computer can't re-launch it")
  })
})
