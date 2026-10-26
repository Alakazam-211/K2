// qrSvg (prd-app-phone-pairing-v1 §8.4): draws pairing.start's QR rows
// with the quiet zone, refuses anything that isn't a square of 0/1 or a
// plain color, and puts nothing unchecked into the markup.

import { describe, expect, it } from 'vitest'

import { qrSvg } from '../src/index.ts'

const ROWS = ['101', '010', '111']

describe('qrSvg', () => {
  it('draws one unit square per dark module inside the quiet zone', () => {
    const svg = qrSvg(ROWS)
    expect(svg.startsWith('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 11 11" width="88" height="88"')).toBe(true)
    expect(svg).toContain('<rect width="11" height="11" fill="#fff"/>')
    const squares = svg.match(/M\d+ \d+h1v1h-1z/g) ?? []
    expect(squares).toHaveLength(6)
    expect(squares[0]).toBe('M4 4h1v1h-1z')
    expect(svg).toContain('M5 5h1v1h-1z')
  })

  it('takes a size, a quiet zone and colors', () => {
    const svg = qrSvg(ROWS, { size: 240, quiet: 0, dark: '#123456', light: 'white' })
    expect(svg).toContain('viewBox="0 0 3 3" width="240" height="240"')
    expect(svg).toContain('fill="#123456"')
    expect(svg).toContain('M0 0h1v1h-1z')
  })

  it('refuses bad rows and colors loudly', () => {
    expect(() => qrSvg([])).toThrow(/square/)
    expect(() => qrSvg(['10', '1'])).toThrow(/square/)
    expect(() => qrSvg(['1x', '01'])).toThrow(/square/)
    expect(() => qrSvg(ROWS, { dark: '"><script>' })).toThrow(/color/)
    expect(() => qrSvg(ROWS, { quiet: -1 })).toThrow(/quiet/)
    expect(() => qrSvg(ROWS, { size: 0 })).toThrow(/size/)
  })
})
