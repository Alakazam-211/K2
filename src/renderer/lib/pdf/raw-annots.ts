// What pdf.js doesn't tell us about an annotation: its /NM (the stable id
// K2 keys it by) and whether it has a cloudy border (/BE << /S /C >>, or the
// /IT /PolygonCloud intent: a revision cloud). We read them from the file's
// own bytes, the binary string fs/read-binary gave us, looking only at
// objects that carry one of those keys.
//
// Limit: objects packed in a compressed object stream (PDF 1.5+) aren't
// scanned, so their annotations fall back to a page-and-index key and their
// cloud is named a polygon. K2 Cores' PDF pack (P3) reads every object.

export interface RawAnnotDict {
  nm?: string
  cloud: boolean
}

/** pdf.js's annotation id for object `num gen`: "12R", or "12R3" for a non-zero generation. */
export const pdfjsId = (num: number, gen: number): string => (gen === 0 ? `${num}R` : `${num}R${gen}`)

function unescapeLiteral(s: string): string {
  return s.replace(/\\(?:([nrtbf()\\])|([0-7]{1,3})|\r?\n)/g, (_m, c: string | undefined, oct: string | undefined) => {
    if (oct) return String.fromCharCode(parseInt(oct, 8) & 0xff)
    if (!c) return ''
    return ({ n: '\n', r: '\r', t: '\t', b: '\b', f: '\f' } as Record<string, string>)[c] ?? c
  })
}

/** A PDF text string's bytes as text: UTF-16BE with a BOM, else PDFDocEncoding (Latin-1 for what /NM holds). */
function decodeText(bytes: string): string {
  if (bytes.charCodeAt(0) === 0xfe && bytes.charCodeAt(1) === 0xff) {
    let out = ''
    for (let i = 2; i + 1 < bytes.length; i += 2) out += String.fromCharCode((bytes.charCodeAt(i) << 8) | bytes.charCodeAt(i + 1))
    return out
  }
  return bytes
}

/** The literal string that starts at `open` (an unescaped "("), balanced parentheses. */
function literalAt(src: string, open: number): string | null {
  let depth = 0
  for (let i = open; i < src.length; i++) {
    const c = src[i]
    if (c === '\\') {
      i++
      continue
    }
    if (c === '(') depth++
    else if (c === ')' && --depth === 0) return src.slice(open + 1, i)
  }
  return null
}

function readNm(dict: string): string | undefined {
  const m = /\/NM\s*([(<])/.exec(dict)
  if (!m) return undefined
  const at = m.index + m[0].length - 1
  if (m[1] === '(') {
    const lit = literalAt(dict, at)
    return lit === null ? undefined : decodeText(unescapeLiteral(lit))
  }
  const end = dict.indexOf('>', at)
  if (end < 0) return undefined
  const hex = dict.slice(at + 1, end).replace(/\s+/g, '')
  let bytes = ''
  for (let i = 0; i < hex.length; i += 2) bytes += String.fromCharCode(parseInt(hex.slice(i, i + 2).padEnd(2, '0'), 16))
  return decodeText(bytes)
}

const CLOUDY = /\/BE\s*<<[^>]*\/S\s*\/C\b|\/IT\s*\/(?:PolygonCloud|PolyLineCloud)\b/

/** Every uncompressed object with an /NM or a cloudy border, by pdf.js id. */
export function scanAnnotDicts(binary: string): Map<string, RawAnnotDict> {
  const out = new Map<string, RawAnnotDict>()
  const look = (from: number): void => {
    // The object holding the key at `from`: the nearest "N G obj" before it.
    let start = -1
    let num = 0
    let gen = 0
    for (let i = binary.lastIndexOf('obj', from); i > 0; i = binary.lastIndexOf('obj', i - 1)) {
      const m = /(\d+)\s+(\d+)\s+$/.exec(binary.slice(Math.max(0, i - 24), i))
      if (!m) continue
      start = i + 3
      num = Number(m[1])
      gen = Number(m[2])
      break
    }
    if (start < 0) return
    const end = binary.indexOf('endobj', start)
    const body = binary.slice(start, end < 0 ? undefined : end)
    // A stream's data isn't the dictionary: a key in it is someone's content.
    const s = body.search(/\bstream\b/)
    const dict = s < 0 ? body : body.slice(0, s)
    if (from >= start + dict.length) return
    const id = pdfjsId(num, gen)
    if (out.has(id)) return
    const nm = readNm(dict)
    const cloud = CLOUDY.test(dict)
    if (nm !== undefined || cloud) out.set(id, nm !== undefined ? { nm, cloud } : { cloud })
  }
  for (const key of [/\/NM\s*[(<]/g, /\/BE\s*<</g, /\/IT\s*\/Poly/g]) {
    for (let m = key.exec(binary); m; m = key.exec(binary)) look(m.index)
  }
  return out
}
