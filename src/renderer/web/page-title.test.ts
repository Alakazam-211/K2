import { afterEach, describe, expect, it, vi } from 'vitest'

const isWebMock = vi.hoisted(() => vi.fn(() => true))

vi.mock('@/lib/is-web', () => ({
  isWebClient: () => isWebMock(),
}))

import {
  k2BasePageTitle,
  k2PageTitle,
  webSubdomainLabel,
} from './page-title'

describe('webSubdomainLabel', () => {
  it('takes the first label of *.app.k2.dev', () => {
    expect(webSubdomainLabel('z3thon.app.k2.dev')).toBe('z3thon')
  })

  it('takes the first label of *.k2.dev', () => {
    expect(webSubdomainLabel('z3thon.k2.dev')).toBe('z3thon')
  })

  it('returns null for empty / IPv4', () => {
    expect(webSubdomainLabel('')).toBeNull()
    expect(webSubdomainLabel('127.0.0.1')).toBeNull()
  })
})

describe('k2BasePageTitle / k2PageTitle', () => {
  afterEach(() => {
    isWebMock.mockReturnValue(true)
  })

  it('formats subdomain | K2 on web', () => {
    expect(k2BasePageTitle('z3thon.app.k2.dev')).toBe('z3thon | K2')
  })

  it('adds zoom suffix', () => {
    expect(k2PageTitle(1.25, 'z3thon.app.k2.dev')).toBe('z3thon | K2 — 125%')
  })

  it('desktop stays plain K2', () => {
    isWebMock.mockReturnValue(false)
    expect(k2BasePageTitle('z3thon.app.k2.dev')).toBe('K2')
    expect(k2PageTitle(1.1, 'z3thon.app.k2.dev')).toBe('K2 — 110%')
  })
})
