import { describe, it, expect } from 'vitest'

// Import the pure helpers directly from fileCategory.ts — no React or
// Tauri deps, so no module-load side effects to mock.
import {
  getFileCategory,
  getDefaultViewMode,
  isBinaryOrMediaCategory,
} from './fileCategory'

describe('getFileCategory — HTML (#587)', () => {
  it('classifies .html and .htm as "html"', () => {
    expect(getFileCategory('/tmp/report.html')).toBe('html')
    expect(getFileCategory('/tmp/index.htm')).toBe('html')
    // Case-insensitive, like the other extension buckets.
    expect(getFileCategory('/tmp/DASH.HTML')).toBe('html')
  })

  it('leaves the other base categories unchanged', () => {
    expect(getFileCategory('/tmp/notes.md')).toBe('markdown')
    expect(getFileCategory('/tmp/pic.png')).toBe('image')
    expect(getFileCategory('/tmp/doc.pdf')).toBe('pdf')
    expect(getFileCategory('/tmp/letter.docx')).toBe('docx')
    expect(getFileCategory('/tmp/main.rs')).toBe('text')
  })
})

describe('getFileCategory — image expansions', () => {
  it('includes heic, avif, tif, tiff', () => {
    expect(getFileCategory('/a/x.heic')).toBe('image')
    expect(getFileCategory('/a/x.avif')).toBe('image')
    expect(getFileCategory('/a/x.tif')).toBe('image')
    expect(getFileCategory('/a/x.TIFF')).toBe('image')
  })

  it('keeps existing image types', () => {
    for (const ext of ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp', 'ico']) {
      expect(getFileCategory(`/a/x.${ext}`)).toBe('image')
    }
  })
})

describe('getFileCategory — csv / media / zip / diagram / binary', () => {
  it('classifies csv and tsv', () => {
    expect(getFileCategory('/data/out.csv')).toBe('csv')
    expect(getFileCategory('/data/out.tsv')).toBe('csv')
    expect(getFileCategory('/data/OUT.CSV')).toBe('csv')
  })

  it('classifies audio', () => {
    for (const ext of ['mp3', 'wav', 'ogg', 'flac', 'm4a', 'aac']) {
      expect(getFileCategory(`/m/a.${ext}`)).toBe('audio')
    }
  })

  it('classifies video', () => {
    for (const ext of ['mp4', 'webm', 'mov', 'mkv']) {
      expect(getFileCategory(`/m/v.${ext}`)).toBe('video')
    }
  })

  it('classifies zip', () => {
    expect(getFileCategory('/pkg/app.zip')).toBe('zip')
  })

  it('classifies mermaid as diagram', () => {
    expect(getFileCategory('/d/flow.mermaid')).toBe('diagram')
    expect(getFileCategory('/d/flow.mmd')).toBe('diagram')
  })

  it('classifies diagram source types', () => {
    expect(getFileCategory('/d/s.drawio')).toBe('diagramSource')
    expect(getFileCategory('/d/s.dio')).toBe('diagramSource')
    expect(getFileCategory('/d/s.puml')).toBe('diagramSource')
    expect(getFileCategory('/d/s.plantuml')).toBe('diagramSource')
    expect(getFileCategory('/d/s.dot')).toBe('diagramSource')
    expect(getFileCategory('/d/s.gv')).toBe('diagramSource')
  })

  it('classifies known binary extensions', () => {
    for (const ext of [
      'wasm',
      'bin',
      'exe',
      'dll',
      'so',
      'dylib',
      'class',
      'o',
      'woff',
      'woff2',
      'ttf',
      'otf',
    ]) {
      expect(getFileCategory(`/b/x.${ext}`)).toBe('binary')
    }
  })

  it('does not special-case xlsx/pptx (non-goals)', () => {
    // Stay as text so we don't pretend to preview Office books/decks.
    expect(getFileCategory('/a/sheet.xlsx')).toBe('text')
    expect(getFileCategory('/a/deck.pptx')).toBe('text')
  })
})

describe('getDefaultViewMode', () => {
  it('defaults HTML to the rendered (preview) view, like markdown', () => {
    expect(getDefaultViewMode('html')).toBe('rendered')
    expect(getDefaultViewMode('markdown')).toBe('rendered')
  })

  it('keeps text/code defaulting to raw', () => {
    expect(getDefaultViewMode('text')).toBe('raw')
  })

  it('defaults csv and diagram types to rendered', () => {
    expect(getDefaultViewMode('csv')).toBe('rendered')
    expect(getDefaultViewMode('diagram')).toBe('rendered')
    expect(getDefaultViewMode('diagramSource')).toBe('rendered')
  })

  it('defaults media, zip, binary, pdf, docx, image to rendered', () => {
    expect(getDefaultViewMode('audio')).toBe('rendered')
    expect(getDefaultViewMode('video')).toBe('rendered')
    expect(getDefaultViewMode('zip')).toBe('rendered')
    expect(getDefaultViewMode('binary')).toBe('rendered')
    expect(getDefaultViewMode('pdf')).toBe('rendered')
    expect(getDefaultViewMode('docx')).toBe('rendered')
    expect(getDefaultViewMode('image')).toBe('rendered')
  })
})

describe('isBinaryOrMediaCategory', () => {
  it('is true for categories that skip fs/read-file text load', () => {
    for (const c of ['image', 'pdf', 'docx', 'audio', 'video', 'zip', 'binary'] as const) {
      expect(isBinaryOrMediaCategory(c)).toBe(true)
    }
  })

  it('is false for text dual-mode types', () => {
    for (const c of [
      'text',
      'markdown',
      'html',
      'csv',
      'diagram',
      'diagramSource',
    ] as const) {
      expect(isBinaryOrMediaCategory(c)).toBe(false)
    }
  })
})
