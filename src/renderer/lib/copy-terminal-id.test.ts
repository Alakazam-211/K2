import { describe, expect, it } from 'vitest'
import { resolveCopyableTerminalId } from './copy-terminal-id'

describe('resolveCopyableTerminalId', () => {
  it('prefers the daemon session UUID from the terminalId→session map', () => {
    const id = resolveCopyableTerminalId(
      [{ type: 'terminal', data: { terminalId: 'local-uuid', command: 'claude' } }],
      { 'local-uuid': 'daemon-session-uuid' },
    )
    expect(id).toBe('daemon-session-uuid')
  })

  it('falls back to -shell mapping used by agent-exit fallback panes', () => {
    const id = resolveCopyableTerminalId(
      [{ type: 'terminal', data: { terminalId: 'local-uuid' } }],
      { 'local-uuid-shell': 'shell-session' },
    )
    expect(id).toBe('shell-session')
  })

  it('falls back to attachAgentName when no live session is registered', () => {
    const id = resolveCopyableTerminalId(
      [{
        type: 'terminal',
        data: { terminalId: 'local-uuid', attachAgentName: 'cortana' },
      }],
      {},
    )
    expect(id).toBe('cortana')
  })

  it('falls back to renderer terminalId (no command required — GA restore path)', () => {
    // Kessel serialize drops `command`; the menu must still offer an id.
    const id = resolveCopyableTerminalId(
      [{ type: 'terminal', data: { terminalId: 'local-uuid' } }],
      {},
    )
    expect(id).toBe('local-uuid')
  })

  it('skips non-terminal items and returns null when none match', () => {
    expect(
      resolveCopyableTerminalId(
        [{ type: 'file-viewer', data: { filePath: '/tmp/x' } }],
        {},
      ),
    ).toBeNull()
    expect(resolveCopyableTerminalId([], {})).toBeNull()
  })
})
