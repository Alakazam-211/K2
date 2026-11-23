// @vitest-environment jsdom
// What a paste carries for a box that takes files (lib/compose-paste).
// Fail loud.
//
//   1. WKWebView's shape (`files`) and Chromium's (`items`, kind "file").
//   2. No files: null (the field pastes the text itself).
//   3. A clipboard image (`image.png`, or no name) gets its own name, so a
//      second screenshot never lands on the first one's upload path.
//   4. Text beside files pastes too, unless it is only their names (a
//      Finder copy) or one URL (a browser's Copy Image).
import { describe, expect, it } from 'vitest'
import { composePaste, filesOfClipboard, pastedImageName } from './compose-paste'

const NOW = new Date(2026, 10, 23, 14, 25, 1)

function clip(opts: { files?: File[]; items?: File[]; text?: string }): Pick<DataTransfer, 'files' | 'items' | 'getData'> {
  const items = (opts.items ?? []).map((f) => ({ kind: 'file', type: f.type, getAsFile: () => f }))
  return {
    files: (opts.files ?? []) as unknown as FileList,
    items: items as unknown as DataTransferItemList,
    getData: (t: string) => (t === 'text/plain' ? opts.text ?? '' : ''),
  }
}

describe('filesOfClipboard', () => {
  it('reads files, else file items; skips string items', () => {
    const a = new File(['a'], 'a.png', { type: 'image/png' })
    expect(filesOfClipboard(clip({ files: [a] }))).toEqual([a])
    const b = new File(['b'], 'b.png', { type: 'image/png' })
    const withString = {
      files: [] as unknown as FileList,
      items: [
        { kind: 'string', type: 'text/plain', getAsFile: () => null },
        { kind: 'file', type: 'image/png', getAsFile: () => b },
      ] as unknown as DataTransferItemList,
    }
    expect(filesOfClipboard(withString)).toEqual([b])
    expect(filesOfClipboard(null)).toEqual([])
  })
})

describe('composePaste', () => {
  it('no files: null, the field pastes the text', () => {
    expect(composePaste(clip({ text: 'hello there' }), NOW)).toBeNull()
  })

  it('a screenshot (WKWebView: files, named image.png) gets its own name and drops the default', () => {
    const shot = new File(['png'], 'image.png', { type: 'image/png' })
    const got = composePaste(clip({ files: [shot] }), NOW)!
    expect(got.paths).toEqual([])
    expect(got.files.map((f) => f.name)).toEqual(['pasted-image-20261123-142501.png'])
    expect(got.files[0].type).toBe('image/png')
    expect(got.keepText).toBe(false)
  })

  it('an image item with no name (Chromium) is named from its type; more in one paste count up', () => {
    const one = new File(['j'], '', { type: 'image/jpeg' })
    const two = new File(['g'], '', { type: 'image/gif' })
    const got = composePaste(clip({ items: [one, two] }), NOW)!
    expect(got.files.map((f) => f.name)).toEqual(['pasted-image-20261123-142501.jpg', 'pasted-image-20261123-142501-2.gif'])
  })

  it('a named file keeps its name', () => {
    const pdf = new File(['%PDF'], 'report.pdf', { type: 'application/pdf' })
    const got = composePaste(clip({ files: [pdf], text: 'report.pdf' }), NOW)!
    expect(got.files).toEqual([pdf])
    // A Finder copy's text is only the file's name: not pasted beside it.
    expect(got.keepText).toBe(false)
  })

  it('an image with real text: both (the text pastes as text)', () => {
    const shot = new File(['png'], 'image.png', { type: 'image/png' })
    const got = composePaste(clip({ files: [shot], text: 'Q3 numbers\n12\t14' }), NOW)!
    expect(got.files).toHaveLength(1)
    expect(got.keepText).toBe(true)
  })

  it("an image with only its address (Copy Image): the address doesn't paste", () => {
    const shot = new File(['png'], 'image.png', { type: 'image/png' })
    expect(composePaste(clip({ files: [shot], text: 'https://example.com/cat.png' }), NOW)!.keepText).toBe(false)
  })
})

describe('pastedImageName', () => {
  it('falls back to the name’s extension, then png', () => {
    expect(pastedImageName(new File(['x'], 'image.TIFF', { type: '' }), NOW, 1)).toBe('pasted-image-20261123-142501.tiff')
    expect(pastedImageName(new File(['x'], '', { type: 'image/x-unknown' }), NOW, 3)).toBe('pasted-image-20261123-142501-3.png')
  })
})
