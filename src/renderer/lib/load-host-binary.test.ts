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

  it('decodes empty string to empty array', () => {
    expect(decodeBase64ToUint8Array('').length).toBe(0)
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
