import { describe, it, expect } from 'vitest'
import { normalizeUrl } from './normalizeUrl'

describe('BrowserPane.normalizeUrl', () => {
  it('prefixes loopback hosts with http://', () => {
    expect(normalizeUrl('127.0.0.1:8788')).toBe('http://127.0.0.1:8788')
    expect(normalizeUrl('localhost:5173')).toBe('http://localhost:5173')
    expect(normalizeUrl('localhost')).toBe('http://localhost')
    expect(normalizeUrl('127.0.0.1')).toBe('http://127.0.0.1')
    expect(normalizeUrl('::1')).toBe('http://[::1]')
    expect(normalizeUrl('[::1]')).toBe('http://[::1]')
    expect(normalizeUrl('[::1]:8788')).toBe('http://[::1]:8788')
  })

  it('prefixes other bare hosts with https://', () => {
    expect(normalizeUrl('example.com')).toBe('https://example.com')
    expect(normalizeUrl('example.com/path')).toBe('https://example.com/path')
  })

  it('leaves explicit schemes unchanged', () => {
    expect(normalizeUrl('https://127.0.0.1')).toBe('https://127.0.0.1')
    expect(normalizeUrl('https://127.0.0.1:8788')).toBe('https://127.0.0.1:8788')
    expect(normalizeUrl('http://example.com')).toBe('http://example.com')
  })

  it('trims whitespace and treats empty as empty', () => {
    expect(normalizeUrl('  127.0.0.1:8788  ')).toBe('http://127.0.0.1:8788')
    expect(normalizeUrl('')).toBe('')
    expect(normalizeUrl('   ')).toBe('')
  })
})
