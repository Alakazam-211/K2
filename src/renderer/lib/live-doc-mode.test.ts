import { describe, expect, it } from 'vitest'
import { liveDocMode } from './live-doc-mode'

const text = (family: string) => ({ tier: 1, pack: 'text', family })

describe('liveDocMode', () => {
  it('maps every text family to a live editor that keeps \\n and binds to source', () => {
    const cases: Array<[string, string, string | null]> = [
      ['text', 'plain', null],
      ['code', 'code', null],
      ['config', 'code', null],
      ['markup', 'code', 'markup'],
      ['diagram', 'code', 'diagram'],
      ['csv-as-text', 'csv-text', 'grid'],
    ]
    for (const [family, editor, preview] of cases) {
      const m = liveDocMode(text(family))
      expect(m.editor, family).toBe(editor)
      expect(m.preview, family).toBe(preview)
      expect(m.live, family).toBe(true)
      expect(m.lineSeparator, family).toBe('\n')
      expect(m.root, family).toBe('source')
    }
  })

  it('maps Markdown to the Markdown editor with a preview', () => {
    const m = liveDocMode({ tier: 1, pack: 'markdown', family: 'markdown' })
    expect(m.editor).toBe('markdown')
    expect(m.preview).toBe('markdown')
    expect(m.lineSeparator).toBe('\n')
  })

  it('checks syntax for config files only', () => {
    expect(liveDocMode(text('config')).checkSyntax).toBe(true)
    expect(liveDocMode(text('code')).checkSyntax).toBe(false)
  })

  it('says CSV is in text mode', () => {
    expect(liveDocMode(text('csv-as-text')).note).toMatch(/CSV as text/)
  })

  it('shows any other file as the any-file view (tier 0), not an editor', () => {
    const m = liveDocMode({ tier: 0, pack: 'blob', family: 'none' })
    expect(m.editor).toBe('any-file')
    expect(m.live).toBe(false)
    expect(m.lineSeparator).toBeNull()
    expect(m.root).toBeNull()
  })

  for (const code of ['too_large', 'not_utf8', 'binary', 'mixed_line_endings']) {
    it(`a refused text file (${code}) is the any-file view with the reason, never a plain editor`, () => {
      const m = liveDocMode({
        tier: 0,
        pack: 'blob',
        family: 'none',
        fallback: { code, message: `file was refused: ${code}` },
      })
      expect(m.editor).toBe('any-file')
      expect(m.live).toBe(false)
      expect(m.note).toBe(`Not live text: file was refused: ${code}`)
    })
  }

  it('never makes an editable buffer from a fallback, even if the tier says live', () => {
    const m = liveDocMode({ tier: 1, pack: 'text', family: 'text', fallback: { code: 'binary' } })
    expect(m.editor).toBe('any-file')
    expect(m.note).toBe('Not live text: binary')
  })

  it('treats an unknown family as plain live text and no answer as the any-file view', () => {
    expect(liveDocMode(text('notebook')).editor).toBe('plain')
    expect(liveDocMode(null).editor).toBe('any-file')
    expect(liveDocMode(undefined).live).toBe(false)
  })
})
