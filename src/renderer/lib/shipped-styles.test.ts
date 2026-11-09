import { describe, expect, it } from 'vitest'
import { STYLES } from '@/styles.generated'

// Rosson, 2026-11-09: this version ships two themes, Terminal and Glass. A
// style in development stays `hidden` in its style.json until it replaces one.
describe('shipped styles', () => {
  it('shows exactly Terminal and Glass in the picker', () => {
    expect(STYLES.filter((s) => !s.hidden).map((s) => s.id).sort()).toEqual(['glass', 'terminal'])
  })
})
