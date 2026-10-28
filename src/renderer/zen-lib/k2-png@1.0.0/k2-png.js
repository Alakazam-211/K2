/*! k2-png 1.0.0 (FSL-1.1-Apache-2.0), K2's widget kit: canvas pixels to PNG without toDataURL. Copyright 2025-2026 Alakazam Labs. */
// K2Png: make a PNG from canvas pixels inside a Garden.
//
//   requires.libs: ["k2-png@1"]
//
//   const bytes = await K2Png.fromCanvas(canvas)          // Uint8Array, a whole PNG file
//   const blob = await K2Png.toBlob(canvas)               // image/png Blob
//   const url = await K2Png.toDataURL(canvas)             // data:image/png;base64,…
//   const bytes = await K2Png.encode(ctx.getImageData(0, 0, w, h))
//
// Never rely on canvas.toDataURL / canvas.toBlob in a Garden: on WebKit
// (the macOS app, iPhone and iPad) they come back empty inside K2's sealed
// widget frame, while getImageData works. K2Png reads the pixels with
// getImageData and writes the PNG itself (8-bit RGBA, no filter), with the
// browser's own CompressionStream('deflate') for the image data, so every
// device makes the same file. Each call answers a Promise.
;(function () {
  'use strict'
  if (window.K2Png) return

  var CRC = (function () {
    var t = new Uint32Array(256)
    for (var n = 0; n < 256; n++) {
      var c = n
      for (var k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
      t[n] = c >>> 0
    }
    return t
  })()

  function crc32(bytes, start, end) {
    var c = 0xffffffff
    for (var i = start; i < end; i++) c = CRC[(c ^ bytes[i]) & 0xff] ^ (c >>> 8)
    return (c ^ 0xffffffff) >>> 0
  }

  function u32(out, at, v) {
    out[at] = (v >>> 24) & 0xff
    out[at + 1] = (v >>> 16) & 0xff
    out[at + 2] = (v >>> 8) & 0xff
    out[at + 3] = v & 0xff
  }

  // One chunk: length, type, data, CRC over type + data.
  function chunk(type, data) {
    var out = new Uint8Array(12 + data.length)
    u32(out, 0, data.length)
    for (var i = 0; i < 4; i++) out[4 + i] = type.charCodeAt(i)
    out.set(data, 8)
    u32(out, 8 + data.length, crc32(out, 4, 8 + data.length))
    return out
  }

  async function deflate(raw) {
    if (typeof CompressionStream !== 'function') throw new Error('K2Png needs CompressionStream (update the browser or app)')
    var stream = new Blob([raw]).stream().pipeThrough(new CompressionStream('deflate'))
    return new Uint8Array(await new Response(stream).arrayBuffer())
  }

  function pixels(source) {
    if (source && typeof source.getContext === 'function') {
      var ctx = source.getContext('2d')
      if (!ctx) throw new Error('K2Png: this canvas has no 2d context (draw on a 2d canvas, or pass ImageData)')
      return ctx.getImageData(0, 0, source.width, source.height)
    }
    if (source && source.data && source.width > 0 && source.height > 0) return source
    throw new Error('K2Png: pass a canvas, ImageData or {width, height, data}')
  }

  async function encode(source) {
    var img = pixels(source)
    var w = img.width
    var h = img.height
    var data = img.data
    if (data.length !== w * h * 4) throw new Error('K2Png: data must be width × height × 4 bytes (RGBA)')
    // Each row: filter byte 0, then the row's RGBA bytes.
    var stride = w * 4
    var raw = new Uint8Array((stride + 1) * h)
    for (var y = 0; y < h; y++) raw.set(data.subarray ? data.subarray(y * stride, (y + 1) * stride) : Array.prototype.slice.call(data, y * stride, (y + 1) * stride), y * (stride + 1) + 1)
    var ihdr = new Uint8Array(13)
    u32(ihdr, 0, w)
    u32(ihdr, 4, h)
    ihdr[8] = 8 // bit depth
    ihdr[9] = 6 // RGBA
    var parts = [new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', await deflate(raw)), chunk('IEND', new Uint8Array(0))]
    var size = 0
    for (var i = 0; i < parts.length; i++) size += parts[i].length
    var out = new Uint8Array(size)
    var at = 0
    for (var j = 0; j < parts.length; j++) {
      out.set(parts[j], at)
      at += parts[j].length
    }
    return out
  }

  async function toBlob(source) {
    return new Blob([await encode(source)], { type: 'image/png' })
  }

  async function toDataURL(source) {
    var bytes = await encode(source)
    var s = ''
    for (var i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, Array.prototype.slice.call(bytes, i, i + 0x8000))
    return 'data:image/png;base64,' + btoa(s)
  }

  window.K2Png = Object.freeze({ version: '1.0.0', encode: encode, fromCanvas: encode, toBlob: toBlob, toDataURL: toDataURL })
})()
