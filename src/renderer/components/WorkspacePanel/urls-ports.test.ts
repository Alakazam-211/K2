// URLs & Ports drawer — pure derivation logic (urls-ports.ts).

import { describe, it, expect } from 'vitest'
import { nestedPublicUrl, sortedTargets, urlCount } from './urls-ports'

describe('nestedPublicUrl', () => {
  it('prefixes the label onto the tunnel public host', () => {
    expect(nestedPublicUrl('staging', 'rosson', 'https://rosson.k2.dev')).toBe(
      'https://staging.rosson.k2.dev',
    )
  })

  it('tolerates a trailing slash on the public URL', () => {
    expect(nestedPublicUrl('staging', '', 'https://rosson.k2.dev/')).toBe(
      'https://staging.rosson.k2.dev',
    )
  })

  it('falls back to primary label + k2.dev when no public URL', () => {
    expect(nestedPublicUrl('staging', 'rosson', null)).toBe(
      'https://staging.rosson.k2.dev',
    )
  })

  it('publicUrl wins over the primary fallback (host truth over label guess)', () => {
    // A daemon on a non-default host suffix must derive from the URL it
    // actually predicted, not the k2.dev fallback.
    expect(nestedPublicUrl('staging', 'rosson', 'https://rosson.custom.example')).toBe(
      'https://staging.rosson.custom.example',
    )
  })

  it('returns null when neither publicUrl nor primary is known (never fabricate)', () => {
    expect(nestedPublicUrl('staging', '', null)).toBeNull()
    expect(nestedPublicUrl('staging', '  ', '')).toBeNull()
  })

  it('returns null for a blank label', () => {
    expect(nestedPublicUrl('', 'rosson', 'https://rosson.k2.dev')).toBeNull()
    expect(nestedPublicUrl('   ', 'rosson', null)).toBeNull()
  })

  it('ignores a non-https publicUrl and uses the primary fallback', () => {
    expect(nestedPublicUrl('staging', 'rosson', 'http://insecure.example')).toBe(
      'https://staging.rosson.k2.dev',
    )
    expect(nestedPublicUrl('staging', '', 'http://insecure.example')).toBeNull()
  })
})

describe('sortedTargets', () => {
  it('sorts rows by label so the table never reshuffles across refreshes', () => {
    expect(
      sortedTargets({ preview: '127.0.0.1:8080', api: 'localhost:4000', staging: 'localhost:3000' }),
    ).toEqual([
      ['api', 'localhost:4000'],
      ['preview', '127.0.0.1:8080'],
      ['staging', 'localhost:3000'],
    ])
  })

  it('empty map yields an empty list', () => {
    expect(sortedTargets({})).toEqual([])
  })
})

describe('urlCount', () => {
  it('counts the running public URL plus every nested target', () => {
    expect(urlCount(true, 'https://rosson.k2.dev', { a: 'x', b: 'y' })).toBe(3)
  })

  it('does not count the public URL when the tunnel is down', () => {
    expect(urlCount(false, 'https://rosson.k2.dev', { a: 'x' })).toBe(1)
  })

  it('does not count a running tunnel without a predicted URL', () => {
    expect(urlCount(true, null, {})).toBe(0)
  })
})
