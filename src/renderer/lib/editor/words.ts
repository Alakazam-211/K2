// The status line's counts (build-editors.md E1b): words in a markdown file
// (markup and fence lines aren't words), the reading time at 230 words a
// minute (never under a minute), and the file's line endings.

const FENCE = /^\s*(```|~~~).*$/gm
const HAS_WORD = /[\p{L}\p{N}]/u

export function countWords(text: string): number {
  let n = 0
  for (const token of text.replace(FENCE, ' ').split(/\s+/)) if (HAS_WORD.test(token)) n++
  return n
}

export function readingMinutes(words: number): number {
  return Math.max(1, Math.round(words / 230))
}

export function lineEnding(text: string): 'LF' | 'CRLF' {
  return text.includes('\r\n') ? 'CRLF' : 'LF'
}
