import { describe, it, expect } from 'vitest'
import { parseDelimited, CSV_PREVIEW_MAX_ROWS } from './csvParse'

describe('parseDelimited', () => {
  it('parses simple CSV rows', () => {
    const { rows, truncated } = parseDelimited('a,b,c\n1,2,3\n')
    expect(truncated).toBe(false)
    expect(rows).toEqual([
      ['a', 'b', 'c'],
      ['1', '2', '3'],
    ])
  })

  it('parses TSV with tab delimiter', () => {
    const { rows } = parseDelimited('name\tage\nAda\t36', { delimiter: '\t' })
    expect(rows).toEqual([
      ['name', 'age'],
      ['Ada', '36'],
    ])
  })

  it('handles quoted fields with commas and doubled quotes', () => {
    const { rows } = parseDelimited('id,note\n1,"hello, world"\n2,"she said ""hi"""\n')
    expect(rows).toEqual([
      ['id', 'note'],
      ['1', 'hello, world'],
      ['2', 'she said "hi"'],
    ])
  })

  it('handles CRLF line endings', () => {
    const { rows } = parseDelimited('a,b\r\n1,2\r\n')
    expect(rows).toEqual([
      ['a', 'b'],
      ['1', '2'],
    ])
  })

  it('truncates at maxRows and sets truncated flag', () => {
    const lines = Array.from({ length: 10 }, (_, i) => `r${i},v${i}`).join('\n')
    const { rows, truncated } = parseDelimited(lines, { maxRows: 3 })
    expect(truncated).toBe(true)
    expect(rows).toHaveLength(3)
    expect(rows[0]).toEqual(['r0', 'v0'])
    expect(rows[2]).toEqual(['r2', 'v2'])
  })

  it('exposes the preview row cap constant used by CsvViewer', () => {
    expect(CSV_PREVIEW_MAX_ROWS).toBe(5000)
  })

  it('returns empty rows for empty input', () => {
    expect(parseDelimited('').rows).toEqual([])
  })
})
