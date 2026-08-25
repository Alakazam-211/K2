import { describe, expect, it } from 'vitest'
import { hostDisplayAddress } from './ServerSwitcher'

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
