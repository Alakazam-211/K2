// prd-widget-size-and-shared-libs-v1 WS12–WS14 (0.46.5): the "Large widget"
// line's numbers and words, read from the daemon's `size`.
import { describe, expect, it } from 'vitest'
import { parseZenCustomWidget } from './zen-custom-payload'
import { parseZenWidgetSize, zenLargeWidgetText, zenSizeText, zenWidgetIsLarge } from './zen-widget-size'

const KB = 1024
const MB = 1024 * 1024
const size = (code: number) => ({ code, codeWarnAt: 512 * KB, codeMax: MB })

describe('zen widget size (WS12–WS14)', () => {
  it('the warning is strictly above 512 KB, as the daemon warns', () => {
    expect(zenWidgetIsLarge(size(512 * KB))).toBe(false)
    expect(zenWidgetIsLarge(size(512 * KB + 1))).toBe(true)
    expect(zenWidgetIsLarge(null)).toBe(false)
    expect(zenLargeWidgetText(size(512 * KB))).toBeNull()
    expect(zenLargeWidgetText(size(740 * KB))).toBe(
      'Large widget: 740 KB of code (limit 1 MB). Ask your agent about splitting it or making it an App.',
    )
  })

  it('sizes read like the daemon’s limit words', () => {
    expect(zenSizeText(MB)).toBe('1 MB')
    expect(zenSizeText(3.5 * MB)).toBe('3.5 MB')
    expect(zenSizeText(512 * KB)).toBe('512 KB')
    expect(zenSizeText(900)).toBe('900 bytes')
  })

  it('an unreadable size is no size (no line), and the payload carries a readable one', () => {
    expect(parseZenWidgetSize(null)).toBeNull()
    expect(parseZenWidgetSize({ code: '9' })).toBeNull()
    expect(parseZenWidgetSize({ code: -1, codeWarnAt: 1, codeMax: 2 })).toBeNull()
    const base = { id: 'b1', column: 0, props: {} }
    const raw = { widget: 'board', requested: [], caps: [], hash: 'h', state: 'ok', origin: 'local' }
    expect(parseZenCustomWidget({ ...raw, size: size(600 * KB) }, base).size).toEqual(size(600 * KB))
    expect('size' in parseZenCustomWidget(raw, base), 'an older daemon: no size field').toBe(false)
  })
})
