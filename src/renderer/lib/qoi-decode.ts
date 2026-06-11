/**
 * Minimal QOI (Quite OK Image) decoder.
 *
 * QOI spec: https://qoiformat.org/qoi-specification.pdf
 * Decodes QOI bytes → RGBA Uint8Array.
 */

const QOI_OP_INDEX = 0x00 // 00xxxxxx
const QOI_OP_DIFF = 0x40 // 01xxxxxx
const QOI_OP_LUMA = 0x80 // 10xxxxxx
const QOI_OP_RUN = 0xc0 // 11xxxxxx
const QOI_OP_RGB = 0xfe
const QOI_OP_RGBA = 0xff

export interface QOIImage {
  data: Uint8Array
  width: number
  height: number
}

function qoiColorHash(r: number, g: number, b: number, a: number): number {
  return (r * 3 + g * 5 + b * 7 + a * 11) & 63
}

export function decodeQOI(bytes: Uint8Array): QOIImage {
  // Header: "qoif" (4 bytes) + width (4) + height (4) + channels (1) + colorspace (1)
  if (bytes.length < 14) {
    throw new Error('QOI: too short for header')
  }

  // Verify magic "qoif"
  if (bytes[0] !== 0x71 || bytes[1] !== 0x6f || bytes[2] !== 0x69 || bytes[3] !== 0x66) {
    throw new Error('QOI: invalid magic')
  }

  const width = (bytes[4] << 24) | (bytes[5] << 16) | (bytes[6] << 8) | bytes[7]
  const height = (bytes[8] << 24) | (bytes[9] << 16) | (bytes[10] << 8) | bytes[11]
  // bytes[12] = channels, bytes[13] = colorspace (unused for decoding)

  const pixelCount = width * height
  const data = new Uint8Array(pixelCount * 4)

  // Running pixel and index table
  let r = 0, g = 0, b = 0, a = 255
  const index = new Uint8Array(64 * 4) // 64 entries × 4 channels

  let p = 14 // byte position in input
  let px = 0 // pixel position in output (in bytes, stride 4)
  const end = pixelCount * 4

  while (px < end && p < bytes.length - 8) {
    const b1 = bytes[p++]

    if (b1 === QOI_OP_RGB) {
      r = bytes[p++]
      g = bytes[p++]
      b = bytes[p++]
    } else if (b1 === QOI_OP_RGBA) {
      r = bytes[p++]
      g = bytes[p++]
      b = bytes[p++]
      a = bytes[p++]
    } else {
      const op = b1 & 0xc0

      if (op === QOI_OP_INDEX) {
        const idx = (b1 & 0x3f) * 4
        r = index[idx]
        g = index[idx + 1]
        b = index[idx + 2]
        a = index[idx + 3]
      } else if (op === QOI_OP_DIFF) {
        r = (r + ((b1 >> 4) & 0x03) - 2) & 0xff
        g = (g + ((b1 >> 2) & 0x03) - 2) & 0xff
        b = (b + (b1 & 0x03) - 2) & 0xff
      } else if (op === QOI_OP_LUMA) {
        const b2 = bytes[p++]
        const vg = (b1 & 0x3f) - 32
        r = (r + vg - 8 + ((b2 >> 4) & 0x0f)) & 0xff
        g = (g + vg) & 0xff
        b = (b + vg - 8 + (b2 & 0x0f)) & 0xff
      } else if (op === QOI_OP_RUN) {
        let run = (b1 & 0x3f) + 1
        while (run-- > 0 && px < end) {
          data[px] = r
          data[px + 1] = g
          data[px + 2] = b
          data[px + 3] = a
          px += 4
        }
        // Update index for run pixel
        const hi = qoiColorHash(r, g, b, a) * 4
        index[hi] = r
        index[hi + 1] = g
        index[hi + 2] = b
        index[hi + 3] = a
        continue // skip the normal write below since run already wrote pixels
      }
    }

    // Write pixel
    data[px] = r
    data[px + 1] = g
    data[px + 2] = b
    data[px + 3] = a
    px += 4

    // Update index
    const hi = qoiColorHash(r, g, b, a) * 4
    index[hi] = r
    index[hi + 1] = g
    index[hi + 2] = b
    index[hi + 3] = a
  }

  return { data, width, height }
}

/**
 * Decode a base64 string to Uint8Array.
 * Uses atob for browser compatibility.
 */
export function base64ToUint8Array(b64: string): Uint8Array {
  const binary = atob(b64)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i)
  }
  return bytes
}
