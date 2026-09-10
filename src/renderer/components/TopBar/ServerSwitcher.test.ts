import { describe, expect, it } from 'vitest'
import {
  defaultSwitcherHighlight,
  hostDisplayAddress,
  serverSwitcherOptions,
} from './ServerSwitcher'
import type { ConnectHost } from '@/stores/connect-host'

const box = (id: string, label: string): ConnectHost =>
  ({
    id,
    label,
    hostname: `${id}.k2.dev`,
    port: 443,
    secure: true,
  }) as ConnectHost

describe('hostDisplayAddress', () => {
  it('omits :443 on https', () => {
    expect(
      hostDisplayAddress({ hostname: 'rosson.k2.dev', port: 443, secure: true }),
    ).toBe('rosson.k2.dev')
  })

  it('keeps a non-443 https port', () => {
    expect(
      hostDisplayAddress({ hostname: 'box.example', port: 8443, secure: true }),
    ).toBe('box.example:8443')
  })

  it('keeps LAN http sticky port', () => {
    expect(
      hostDisplayAddress({ hostname: '192.168.1.50', port: 60710, secure: false }),
    ).toBe('192.168.1.50:60710')
  })
})

describe('serverSwitcherOptions + defaultSwitcherHighlight', () => {
  const a = box('a', 'Alpha')
  const b = box('b', 'Beta')

  it('puts Local first, then the filtered hosts', () => {
    expect(serverSwitcherOptions(true, [a, b]).map((o) => (o === 'local' ? 'local' : o.id))).toEqual([
      'local',
      'a',
      'b',
    ])
    expect(serverSwitcherOptions(false, [b]).map((o) => (o === 'local' ? 'local' : o.id))).toEqual(['b'])
  })

  it('search highlights the top-most match, not the connected host', () => {
    const opts = serverSwitcherOptions(false, [a, b])
    expect(defaultSwitcherHighlight(opts, b, true)).toBe(0)
    expect(opts[0]).toBe(a)
  })

  it('idle open highlights the currently connected host', () => {
    const opts = serverSwitcherOptions(true, [a, b])
    expect(defaultSwitcherHighlight(opts, 'local', false)).toBe(0)
    expect(defaultSwitcherHighlight(opts, b, false)).toBe(2)
  })
})
