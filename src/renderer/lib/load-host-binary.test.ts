import { describe, it, expect, vi, beforeEach } from 'vitest'

const daemonCliGet = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
}))

import {
  decodeBase64ToUint8Array,
  loadHostBinary,
  imageMimeFromPath,
  looksLikeBinaryText,
  revokeObjectUrl,
} from './load-host-binary'

describe('decodeBase64ToUint8Array', () => {
  it('decodes ASCII payload', () => {
    // "ABC" in base64
    const bytes = decodeBase64ToUint8Array('QUJD')
    expect(Array.from(bytes)).toEqual([65, 66, 67])
  })

  it('rejects empty base64 (would paint a broken image otherwise)', () => {
    expect(() => decodeBase64ToUint8Array('')).toThrow(/empty or missing base64/)
  })

  it('round-trips binary-ish bytes including NUL', () => {
    // \x00\x01\xff → base64
    const original = new Uint8Array([0, 1, 255])
    let binary = ''
    for (let i = 0; i < original.length; i++) binary += String.fromCharCode(original[i])
    const b64 = btoa(binary)
    expect(Array.from(decodeBase64ToUint8Array(b64))).toEqual([0, 1, 255])
  })
})

describe('loadHostBinary', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
  })

  it('calls fs/read-binary and decodes base64', async () => {
    daemonCliGet.mockResolvedValueOnce({ base64: 'aGVsbG8=' }) // "hello"
    const bytes = await loadHostBinary('/host/pic.png')
    expect(daemonCliGet).toHaveBeenCalledWith('fs/read-binary', { path: '/host/pic.png' })
    expect(new TextDecoder().decode(bytes)).toBe('hello')
  })

  it('propagates daemon errors', async () => {
    daemonCliGet.mockRejectedValueOnce(new Error('payload too large'))
    await expect(loadHostBinary('/big.bin')).rejects.toThrow('payload too large')
  })
})

describe('imageMimeFromPath', () => {
  it('maps common and expanded image extensions', () => {
    expect(imageMimeFromPath('/a/b.png')).toBe('image/png')
    expect(imageMimeFromPath('/a/b.JPG')).toBe('image/jpeg')
    expect(imageMimeFromPath('/a/b.webp')).toBe('image/webp')
    expect(imageMimeFromPath('/a/b.svg')).toBe('image/svg+xml')
    expect(imageMimeFromPath('/a/b.heic')).toBe('image/heic')
    expect(imageMimeFromPath('/a/b.avif')).toBe('image/avif')
    expect(imageMimeFromPath('/a/b.tif')).toBe('image/tiff')
    expect(imageMimeFromPath('/a/b.tiff')).toBe('image/tiff')
    expect(imageMimeFromPath('/a/b.ico')).toBe('image/x-icon')
  })

  it('falls back for unknown extensions', () => {
    expect(imageMimeFromPath('/a/b.xyz')).toBe('application/octet-stream')
  })
})

describe('looksLikeBinaryText', () => {
  it('returns false for normal source', () => {
    expect(looksLikeBinaryText('fn main() {}\n')).toBe(false)
  })

  it('returns true when a NUL is present in the sample', () => {
    expect(looksLikeBinaryText('hello\0world')).toBe(true)
  })

  it('respects sample window', () => {
    const late = 'x'.repeat(100) + '\0'
    expect(looksLikeBinaryText(late, 50)).toBe(false)
    expect(looksLikeBinaryText(late, 200)).toBe(true)
  })
})

describe('revokeObjectUrl', () => {
  it('revokes blob: URLs only', () => {
    const revoke = vi.fn()
    const orig = URL.revokeObjectURL
    URL.revokeObjectURL = revoke
    try {
      revokeObjectUrl('blob:http://localhost/abc')
      revokeObjectUrl('data:image/png;base64,xx')
      revokeObjectUrl(null)
      revokeObjectUrl(undefined)
      expect(revoke).toHaveBeenCalledTimes(1)
      expect(revoke).toHaveBeenCalledWith('blob:http://localhost/abc')
    } finally {
      URL.revokeObjectURL = orig
    }
  })
})

import { sniffImageMime, coerceImageBytesForPreview, bytesToObjectUrl } from './load-host-binary'

describe('sniffImageMime / coerceImageBytesForPreview', () => {
  it('sniffs PNG magic', () => {
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0])
    expect(sniffImageMime(png)).toBe('image/png')
    expect(coerceImageBytesForPreview(png).mime).toBe('image/png')
  })

  it('sniffs JPEG magic', () => {
    const jpg = new Uint8Array([0xff, 0xd8, 0xff, 0xe0, 0, 0, 0, 0])
    expect(sniffImageMime(jpg)).toBe('image/jpeg')
  })

  it('extracts PNG from a synthetic ICO that embeds PNG', () => {
    // Minimal ICO: 1 entry pointing at a PNG blob
    const png = new Uint8Array([
      0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 1, 2, 3,
    ])
    const ico = new Uint8Array(6 + 16 + png.length)
    ico[0] = 0
    ico[1] = 0
    ico[2] = 1 // type icon
    ico[3] = 0
    ico[4] = 1 // count
    ico[5] = 0
    // entry at 6: width/height etc — size + offset at +8 and +12
    const size = png.length
    const offset = 22
    ico[6 + 8] = size & 0xff
    ico[6 + 9] = (size >> 8) & 0xff
    ico[6 + 12] = offset & 0xff
    ico[6 + 13] = (offset >> 8) & 0xff
    ico.set(png, offset)
    const coerced = coerceImageBytesForPreview(ico)
    expect(coerced.mime).toBe('image/png')
    expect(Array.from(coerced.bytes.slice(0, 4))).toEqual([0x89, 0x50, 0x4e, 0x47])
  })
})
