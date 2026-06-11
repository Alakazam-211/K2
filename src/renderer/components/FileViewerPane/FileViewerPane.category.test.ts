import { describe, it, expect } from 'vitest'

// Import the pure helpers directly from fileCategory.ts — no React or
// Tauri deps, so no module-load side effects to mock.
import { getFileCategory, getDefaultViewMode } from './fileCategory'

describe('getFileCategory — HTML (#587)', () => {
  it('classifies .html and .htm as "html"', () => {
    expect(getFileCategory('/tmp/report.html')).toBe('html')
    expect(getFileCategory('/tmp/index.htm')).toBe('html')
    // Case-insensitive, like the other extension buckets.
    expect(getFileCategory('/tmp/DASH.HTML')).toBe('html')
  })

  it('leaves the other categories unchanged', () => {
    expect(getFileCategory('/tmp/notes.md')).toBe('markdown')
    expect(getFileCategory('/tmp/pic.png')).toBe('image')
    expect(getFileCategory('/tmp/doc.pdf')).toBe('pdf')
    expect(getFileCategory('/tmp/letter.docx')).toBe('docx')
    expect(getFileCategory('/tmp/main.rs')).toBe('text')
  })
})

describe('getDefaultViewMode — HTML (#587)', () => {
  it('defaults HTML to the rendered (preview) view, like markdown', () => {
    expect(getDefaultViewMode('html')).toBe('rendered')
    expect(getDefaultViewMode('markdown')).toBe('rendered')
  })

  it('keeps text/code defaulting to raw', () => {
    expect(getDefaultViewMode('text')).toBe('raw')
  })
})
