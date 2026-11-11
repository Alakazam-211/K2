// @vitest-environment jsdom
//
// A copy or cut in the file tree also puts the paths on the OS clipboard as
// text, quoted like a drop (lib/drop-payload.ts) so a paste in a terminal
// gives the same text a drop would. A name with a control character is not
// put there: a pasted line break would run what follows it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { COPY_AS_TEXT_REFUSAL, useFileClipboardStore } from './file-clipboard'
import { useToastStore } from './toast'

const writeText = vi.fn(async (_text: string) => undefined)

beforeEach(() => {
  writeText.mockClear()
  vi.stubGlobal('navigator', { clipboard: { writeText } })
  useToastStore.setState({ toasts: [] })
  useFileClipboardStore.getState().clear()
})
afterEach(() => {
  vi.unstubAllGlobals()
})

describe('file clipboard → OS clipboard text', () => {
  it('copy: backslash-escaped paths, images quoted, no trailing space', () => {
    useFileClipboardStore.getState().copy(['/Users/a/my notes.txt', '/Users/a/Screen Shot.png', '/Users/a/b.md'])
    expect(writeText).toHaveBeenCalledWith("/Users/a/my\\ notes.txt '/Users/a/Screen Shot.png' /Users/a/b.md")
  })

  it('cut: the same text', () => {
    useFileClipboardStore.getState().cut(['/tmp/R&D.txt'])
    expect(writeText).toHaveBeenCalledWith('/tmp/R\\&D.txt')
    expect(useFileClipboardStore.getState().mode).toBe('cut')
  })

  it('a name with a line break: nothing on the OS clipboard, a toast, the in-app copy still holds', () => {
    useFileClipboardStore.getState().copy(['/tmp/ok.txt', '/tmp/notes\nrm -rf ~'])
    expect(writeText).not.toHaveBeenCalled()
    expect(useToastStore.getState().toasts.map((t) => [t.message, t.type])).toEqual([
      [COPY_AS_TEXT_REFUSAL, 'warning'],
    ])
    expect(useFileClipboardStore.getState().paths).toEqual(['/tmp/ok.txt', '/tmp/notes\nrm -rf ~'])
  })
})
