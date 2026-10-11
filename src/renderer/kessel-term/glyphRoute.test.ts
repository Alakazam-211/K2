// The one presentation decision both painters share (glyphRoute.ts).
// rowRender.test.tsx and webgl/glyphAtlas.test.ts pin that each
// painter actually follows it.

import { describe, expect, it } from 'vitest'
import { glyphRoute, VS15 } from './glyphRoute'

describe('glyphRoute', () => {
  it('⏺ (U+23FA) is painted, never a font glyph', () => {
    expect(glyphRoute('⏺')).toEqual({
      kind: 'synthetic',
      spec: { kind: 'symbol', shape: 'circle' },
    })
    for (const ch of ['⏸', '⏹', '⏴', '⏵', '⏶', '⏷']) {
      expect(glyphRoute(ch).kind, ch).toBe('synthetic')
    }
  })

  it('real emoji keep the emoji path (✅ 🎉 😀 ❤)', () => {
    for (const ch of ['✅', '🎉', '😀', '❤']) {
      expect(glyphRoute(ch), ch).toEqual({ kind: 'emoji' })
    }
  })

  it('one-column symbols with an emoji form draw as text + VS15', () => {
    for (const ch of ['✔', '☀', '⏯', '↔', '⚠', '✳']) {
      expect(glyphRoute(ch), ch).toEqual({ kind: 'text', draw: ch + VS15 })
    }
  })

  it('an explicit follower is the app\'s call: no VS15 added', () => {
    expect(glyphRoute('✔️')).toEqual({ kind: 'text', draw: '✔️' })
    expect(glyphRoute('⏺️').kind).toBe('text') // decorated: font path
  })

  it('plain text and glyphs Meslo covers pass through untouched', () => {
    for (const ch of ['a', '●', '○', '✓', '✗', '⠋', 'あ']) {
      expect(glyphRoute(ch), ch).toEqual({ kind: 'text', draw: ch })
    }
    expect(glyphRoute('─').kind).toBe('synthetic')
  })
})
