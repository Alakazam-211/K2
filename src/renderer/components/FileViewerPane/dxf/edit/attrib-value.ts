// An attribute value's (or a single-line text's) checks as you type, by
// the CAD pack's rule for a single-line value: one line, no control characters, no `\U+` (the file
// reads it as a code), and at most 255 bytes as the file writes it. From
// DXF 2007 on that is UTF-8; before, the Windows-1252 code page, where a
// character it lacks is written `\U+XXXX` (7 bytes) and one past U+FFFF
// can't be written at all. A value starting `--` is held back too: the
// kit reads its own flags anywhere in the call.

const LIMIT = 255

// Windows-1252's 0x80–0x9F, as code points (U+FFFD where it has none).
const CP1252_HIGH = [
  0x20ac, 0xfffd, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160, 0x2039, 0x0152, 0xfffd, 0x017d, 0xfffd,
  0xfffd, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x02dc, 0x2122, 0x0161, 0x203a, 0x0153, 0xfffd, 0x017e, 0x0178,
]

/** Bytes as written, or null when a character can't be written in this file. */
export function writtenBytes(value: string, utf8: boolean): number | null {
  if (utf8) return new TextEncoder().encode(value).length
  let n = 0
  for (const ch of value) {
    const u = ch.codePointAt(0)!
    if (u < 0x80 || (u >= 0xa0 && u <= 0xff) || (u !== 0xfffd && CP1252_HIGH.includes(u))) n += 1
    else if (u <= 0xffff) n += 7
    else return null
  }
  return n
}

/** What's wrong with a value, in plain words, or null when it can be written; `text` when it's a text's value, not a label's. */
export function attribProblem(value: string, opts: { utf8: boolean; text?: boolean }): string | null {
  const noun = opts.text ? 'Text' : 'A label'
  const holds = opts.text ? 'a line of text holds' : 'a label holds'
  if (/[\r\n\u2028\u2029]/.test(value)) return 'Keep it to one line.'
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(value)) return 'Remove the hidden character, such as a tab.'
  if (/\\u\+/i.test(value)) return 'Remove “\\U+”. The file would read it as a character code.'
  if (value.startsWith('--')) return `${noun} can’t start with “--” yet.`
  const n = writtenBytes(value, opts.utf8)
  if (n === null) return 'One of these characters can’t be saved in this drawing. Remove it.'
  // eslint-disable-next-line no-control-regex
  if (n > LIMIT) return `Too long: ${n} bytes, and ${holds} ${LIMIT}.${/[^\u0000-\u007f]/.test(value) ? ' Some characters take more than one byte.' : ''}`
  return null
}

/** Whether a drawing writes text as UTF-8: DXF 2007 (AC1021) and later. */
export const writesUtf8 = (acadver: string | undefined): boolean => !acadver || !/^AC\d{4}$/.test(acadver) || acadver >= 'AC1021'
