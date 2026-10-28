// k2-png@1 (prd-gardens-on-phone-v1 Q11, GP-A18): canvas pixels to a PNG
// without toDataURL / toBlob, which come back empty inside K2's sealed frame
// on WebKit. The real checked-in file runs here; the PNG it writes is parsed
// back (signature, chunk CRCs, IHDR, zlib-inflated rows) and must give the
// same pixels. Node has CompressionStream, Blob and Response like a browser.
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { inflateSync } from 'node:zlib'
import { beforeAll, describe, expect, it } from 'vitest'
import { ZEN_LIB_MANIFEST, zenLibResolve } from './zen-lib-loader'

const LIB_DIR = resolve(__dirname, '../../zen-lib')

type Png = {
  version: string
  encode(src: { width: number; height: number; data: Uint8ClampedArray | Uint8Array }): Promise<Uint8Array>
  toBlob(src: { width: number; height: number; data: Uint8ClampedArray | Uint8Array }): Promise<Blob>
  toDataURL(src: { width: number; height: number; data: Uint8ClampedArray | Uint8Array }): Promise<string>
}

let K2Png: Png

function crc32(b: Uint8Array): number {
  let c = 0xffffffff
  for (const x of b) {
    c ^= x
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  }
  return (c ^ 0xffffffff) >>> 0
}

function parse(png: Uint8Array): { width: number; height: number; rgba: Uint8Array } {
  expect(Array.from(png.slice(0, 8))).toEqual([137, 80, 78, 71, 13, 10, 26, 10])
  const dv = new DataView(png.buffer, png.byteOffset, png.byteLength)
  let at = 8
  let width = 0
  let height = 0
  const idat: Uint8Array[] = []
  const types: string[] = []
  while (at < png.length) {
    const len = dv.getUint32(at)
    const type = String.fromCharCode(...png.slice(at + 4, at + 8))
    const data = png.slice(at + 8, at + 8 + len)
    expect(dv.getUint32(at + 8 + len), `${type} CRC`).toBe(crc32(png.slice(at + 4, at + 8 + len)))
    types.push(type)
    if (type === 'IHDR') {
      width = new DataView(data.buffer).getUint32(0)
      height = new DataView(data.buffer).getUint32(4)
      expect([data[8], data[9], data[10], data[11], data[12]]).toEqual([8, 6, 0, 0, 0])
    }
    if (type === 'IDAT') idat.push(data)
    at += 12 + len
  }
  expect(types).toEqual(['IHDR', 'IDAT', 'IEND'])
  const raw = new Uint8Array(inflateSync(Buffer.concat(idat)))
  const stride = width * 4
  expect(raw.length).toBe((stride + 1) * height)
  const rgba = new Uint8Array(stride * height)
  for (let y = 0; y < height; y++) {
    expect(raw[y * (stride + 1)], 'filter 0').toBe(0)
    rgba.set(raw.slice(y * (stride + 1) + 1, (y + 1) * (stride + 1)), y * stride)
  }
  return { width, height, rgba }
}

beforeAll(() => {
  const r = zenLibResolve(['k2-png@1'])
  expect(r.findings.filter((f) => f.error)).toEqual([])
  expect(r.keys).toEqual(['k2-png@1.0.0'])
  const row = ZEN_LIB_MANIFEST.libs.find((l) => l.id === 'k2-png')
  if (!row) throw new Error('no k2-png row in zen-lib.json')
  expect(row.source).toBe('bundled')
  const text = readFileSync(resolve(LIB_DIR, 'k2-png@1.0.0', 'k2-png.js'), 'utf8')
  expect(Buffer.byteLength(text)).toBe(row.files[0].bytes)
  expect(createHash('sha256').update(text).digest('hex')).toBe(row.files[0].sha256)
  const win: Record<string, unknown> = {}
  new Function('window', text)(win)
  K2Png = win.K2Png as Png
})

describe('k2-png@1', () => {
  it('writes a valid PNG whose pixels are the pixels it was given', async () => {
    const width = 3
    const height = 2
    const data = new Uint8ClampedArray(width * height * 4)
    for (let i = 0; i < data.length; i++) data[i] = (i * 37) & 0xff
    const png = await K2Png.encode({ width, height, data })
    const back = parse(png)
    expect([back.width, back.height]).toEqual([width, height])
    expect(Array.from(back.rgba)).toEqual(Array.from(data))
  })

  it('gives a Blob and a data URL of the same bytes', async () => {
    const src = { width: 1, height: 1, data: new Uint8ClampedArray([255, 0, 0, 255]) }
    const png = await K2Png.encode(src)
    const blob = await K2Png.toBlob(src)
    expect(blob.type).toBe('image/png')
    expect(new Uint8Array(await blob.arrayBuffer())).toEqual(png)
    const url = await K2Png.toDataURL(src)
    expect(url.startsWith('data:image/png;base64,')).toBe(true)
    expect(new Uint8Array(Buffer.from(url.slice(22), 'base64'))).toEqual(png)
  })

  it('refuses what is not pixels', async () => {
    await expect(K2Png.encode({ width: 2, height: 2, data: new Uint8ClampedArray(3) })).rejects.toThrow(/width × height × 4/)
    await expect(K2Png.encode({} as never)).rejects.toThrow(/canvas, ImageData/)
  })
})
