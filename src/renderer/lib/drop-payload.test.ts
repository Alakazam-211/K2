// The one builder for path text: external drops, in-app drags, compose
// drafts and copies all come here (lib/drop-payload.ts). Pins the quoting
// per shell, the image rules Claude Code's `[Image #N]` needs, and the
// refusal of names with control characters.

import { describe, expect, it } from 'vitest'
import {
  BRACKETED_PASTE_END,
  BRACKETED_PASTE_START,
  CMD_PERCENT_REFUSAL,
  CONTROL_CHAR_REFUSAL,
  bracketPaste,
  buildComposeDropPayload,
  buildTerminalDropPayload,
  dropShellFor,
  formatDropPaths,
  hasControlChar,
  isImagePath,
  isWindowsPath,
  quoteDropPath,
  quotePathForImageDrop,
  type DropText,
} from './drop-payload'

/** The text of an accepted payload; fails the test on a refusal. */
function text(r: DropText): string {
  if (!r.ok) throw new Error(`refused: ${r.refusal}`)
  return r.text
}

const REFUSED: DropText = { ok: false, refusal: CONTROL_CHAR_REFUSAL }

describe('isImagePath', () => {
  it('knows the image and PDF extensions Claude Code reads, in any case', () => {
    for (const ext of ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'heic', 'heif', 'pdf']) {
      expect(isImagePath(`/tmp/shot.${ext}`), ext).toBe(true)
      expect(isImagePath(`/tmp/SHOT.${ext.toUpperCase()}`), ext.toUpperCase()).toBe(true)
    }
  })

  it('goes by the last extension only', () => {
    expect(isImagePath('/tmp/notes.txt')).toBe(false)
    expect(isImagePath('/tmp/shot.png.txt')).toBe(false)
    expect(isImagePath('/tmp/png')).toBe(false)
    expect(isImagePath('/tmp/archive.tar.gz')).toBe(false)
  })
})

describe('quotePathForImageDrop', () => {
  it('leaves a plain path bare', () => {
    expect(quotePathForImageDrop('/tmp/shot-1_a.png')).toBe('/tmp/shot-1_a.png')
  })

  it('single-quotes a path with spaces and keeps it literal (no backslashes)', () => {
    expect(quotePathForImageDrop('/Users/a/Screen Shot 2026.png')).toBe("'/Users/a/Screen Shot 2026.png'")
  })

  it('closes and reopens the quotes around an apostrophe', () => {
    expect(quotePathForImageDrop("/tmp/Bob's shot.png")).toBe("'/tmp/Bob'\\''s shot.png'")
  })
})

describe('bracketPaste', () => {
  it('wraps the text in the bracketed-paste start and end sequences', () => {
    expect(BRACKETED_PASTE_START).toBe('\x1b[200~')
    expect(BRACKETED_PASTE_END).toBe('\x1b[201~')
    expect(bracketPaste('/tmp/a.png ')).toBe('\x1b[200~/tmp/a.png \x1b[201~')
  })
})

describe('buildTerminalDropPayload (POSIX host)', () => {
  it('joins plain paths with a trailing space', () => {
    expect(text(buildTerminalDropPayload(['/tmp/a.txt', '/tmp/b.txt']))).toBe('/tmp/a.txt /tmp/b.txt ')
  })

  it('wraps image drops in bracketed paste', () => {
    expect(text(buildTerminalDropPayload(['/tmp/shot.png']))).toBe(bracketPaste('/tmp/shot.png '))
  })

  it('quotes an image path (Claude Code fs.exists() it) and backslash-escapes the rest', () => {
    expect(text(buildTerminalDropPayload(['/Users/a/Screen Shot.png', '/Users/a/my notes.txt']))).toBe(
      bracketPaste("'/Users/a/Screen Shot.png' /Users/a/my\\ notes.txt "),
    )
    expect(text(buildTerminalDropPayload(['/Users/a/my notes.txt']))).toBe('/Users/a/my\\ notes.txt ')
  })

  it('backslash-escapes every shell special in a non-image path', () => {
    const specials = ` '"\\()&|;<>$\`!#*?[]{}~`
    const expected = specials
      .split('')
      .map((c) => `\\${c}`)
      .join('')
    expect(text(buildTerminalDropPayload([`/tmp/${specials}.txt`]))).toBe(`/tmp/${expected}.txt `)
  })

  it('escapes $ and backticks so a dropped name cannot expand or run a command', () => {
    expect(text(buildTerminalDropPayload(['/tmp/$HOME `id`.log']))).toBe('/tmp/\\$HOME\\ \\`id\\`.log ')
  })

  it('escapes a leading ~ so the shell does not expand it', () => {
    expect(text(buildTerminalDropPayload(['~/notes.txt']))).toBe('\\~/notes.txt ')
  })

  it('leaves unicode, dashes, dots and commas as they are', () => {
    expect(text(buildTerminalDropPayload(['/tmp/résumé-v1.2,final 日本.txt']))).toBe(
      '/tmp/résumé-v1.2,final\\ 日本.txt ',
    )
  })

  it('single-quotes an image path with a quote in it, POSIX style', () => {
    expect(text(buildTerminalDropPayload(["/tmp/it's.png"]))).toBe(bracketPaste("'/tmp/it'\\''s.png' "))
  })

  it('leaves a safe image path bare and matches image extensions case-insensitively', () => {
    expect(text(buildTerminalDropPayload(['/tmp/SHOT.PNG', '/tmp/doc.pdf']))).toBe(
      bracketPaste('/tmp/SHOT.PNG /tmp/doc.pdf '),
    )
  })
})

describe('control characters are refused', () => {
  it('refuses a line break: "notes\\nrm -rf ~" would run the second line', () => {
    expect(buildTerminalDropPayload(['/tmp/notes\nrm -rf ~'])).toEqual(REFUSED)
    expect(CONTROL_CHAR_REFUSAL).toBe("Can't drop a file whose name has a line break in it.")
  })

  it('refuses the whole drop when one path of several has one', () => {
    expect(buildTerminalDropPayload(['/tmp/ok.txt', '/tmp/bad\r.txt'])).toEqual(REFUSED)
  })

  it('refuses every C0 control, DEL and every C1 control', () => {
    const codes = [
      ...Array.from({ length: 0x20 }, (_, i) => i),
      0x7f,
      ...Array.from({ length: 0x20 }, (_, i) => 0x80 + i),
    ]
    for (const code of codes) {
      const p = `/tmp/a${String.fromCharCode(code)}b.txt`
      expect(hasControlChar(p), code.toString(16)).toBe(true)
      expect(buildTerminalDropPayload([p]), code.toString(16)).toEqual(REFUSED)
    }
  })

  it('refuses in an image drop, a compose draft and clipboard text too', () => {
    expect(buildTerminalDropPayload(['/tmp/a\x1b[31m.png'])).toEqual(REFUSED)
    expect(buildComposeDropPayload(['/tmp/a\tb.txt'])).toEqual(REFUSED)
    expect(formatDropPaths(['/tmp/a\u0000b.txt'], { trailingSpace: false })).toEqual(REFUSED)
  })

  it('refuses a Windows path with a control character', () => {
    expect(buildTerminalDropPayload(['C:\\tmp\\a\nb.txt'], 'cmd')).toEqual(REFUSED)
  })

  it('accepts printable characters next to the control ranges', () => {
    for (const ch of [' ', '~', '\u00a0', '\u00e9']) {
      expect(hasControlChar(`/tmp/a${ch}b`), JSON.stringify(ch)).toBe(false)
    }
  })
})

describe('Windows paths', () => {
  it('knows a drive path and a UNC path; a POSIX path is not one', () => {
    expect(isWindowsPath('C:\\Users\\a.txt')).toBe(true)
    expect(isWindowsPath('d:/work/a.txt')).toBe(true)
    expect(isWindowsPath('\\\\server\\share\\a.txt')).toBe(true)
    expect(isWindowsPath('\\\\?\\C:\\long\\a.txt')).toBe(true)
    expect(isWindowsPath('/Users/a.txt')).toBe(false)
    expect(isWindowsPath('C:relative.txt')).toBe(false)
  })

  it('picks the shell family from the paths; the caller names the Windows shell', () => {
    expect(dropShellFor(['/tmp/a'])).toBe('posix')
    expect(dropShellFor(['C:\\a'])).toBe('powershell')
    expect(dropShellFor(['C:\\a'], 'cmd')).toBe('cmd')
    expect(dropShellFor(['/tmp/a'], 'cmd')).toBe('posix')
  })

  it('PowerShell: a safe path stays bare, others single-quoted, never backslash-escaped', () => {
    expect(text(buildTerminalDropPayload(['C:\\Users\\a\\notes.txt']))).toBe('C:\\Users\\a\\notes.txt ')
    expect(text(buildTerminalDropPayload(['C:\\Users\\a b\\x.txt']))).toBe("'C:\\Users\\a b\\x.txt' ")
  })

  it('PowerShell: $ and backticks are literal inside single quotes', () => {
    expect(quoteDropPath('C:\\tmp\\$env:HOME `x`.txt', 'powershell')).toBe("'C:\\tmp\\$env:HOME `x`.txt'")
  })

  it('PowerShell: an apostrophe, and the curly quotes PowerShell also reads as one, are doubled', () => {
    expect(quoteDropPath("C:\\tmp\\Bob's.txt", 'powershell')).toBe("'C:\\tmp\\Bob''s.txt'")
    expect(quoteDropPath('C:\\tmp\\Bob\u2019s.txt', 'powershell')).toBe("'C:\\tmp\\Bob\u2019\u2019s.txt'")
    expect(quoteDropPath('C:\\tmp\\\u2018a\u201a\u201b.txt', 'powershell')).toBe(
      "'C:\\tmp\\\u2018\u2018a\u201a\u201a\u201b\u201b.txt'",
    )
  })

  it('cmd: a safe path stays bare, others double-quoted as they are', () => {
    expect(text(buildTerminalDropPayload(['C:\\Users\\a\\notes.txt'], 'cmd'))).toBe('C:\\Users\\a\\notes.txt ')
    expect(text(buildTerminalDropPayload(['C:\\Users\\a b\\R&D (1).txt'], 'cmd'))).toBe(
      '"C:\\Users\\a b\\R&D (1).txt" ',
    )
  })

  it('cmd: a name with a % sign is refused (cmd expands %VAR% even in quotes)', () => {
    const refused: DropText = { ok: false, refusal: CMD_PERCENT_REFUSAL }
    expect(CMD_PERCENT_REFUSAL).toBe("Can't drop a file whose name has a % sign into this terminal.")
    expect(buildTerminalDropPayload(['C:\\tmp\\100%PATH%.txt'], 'cmd')).toEqual(refused)
    expect(buildTerminalDropPayload(['C:\\tmp\\ok.txt', 'C:\\tmp\\50%.png'], 'cmd')).toEqual(refused)
  })

  it('a % sign passes in PowerShell and in a POSIX shell', () => {
    expect(text(buildTerminalDropPayload(['C:\\tmp\\100%PATH%.txt'], 'powershell'))).toBe("'C:\\tmp\\100%PATH%.txt' ")
    expect(text(buildTerminalDropPayload(['/tmp/100%PATH%.txt'], 'cmd'))).toBe('/tmp/100%PATH%.txt ')
    expect(text(buildTerminalDropPayload(['/tmp/100%.txt']))).toBe('/tmp/100%.txt ')
  })

  it('an image keeps the bracketed paste, quoted the Windows way (Claude Code strips quotes)', () => {
    expect(text(buildTerminalDropPayload(['C:\\Users\\a\\Screen Shot.png', 'C:\\Users\\a\\b.txt']))).toBe(
      bracketPaste("'C:\\Users\\a\\Screen Shot.png' C:\\Users\\a\\b.txt "),
    )
    expect(text(buildTerminalDropPayload(['C:\\shot.png'], 'cmd'))).toBe(bracketPaste('C:\\shot.png '))
  })
})

describe('buildComposeDropPayload', () => {
  it('joins paths with a trailing space and no bracketed paste', () => {
    expect(text(buildComposeDropPayload(['/tmp/my notes.txt', '/tmp/Screen Shot.png']))).toBe(
      "/tmp/my\\ notes.txt '/tmp/Screen Shot.png' ",
    )
  })

  it('is empty for no paths', () => {
    expect(text(buildComposeDropPayload([]))).toBe('')
  })
})

describe('formatDropPaths', () => {
  it('can leave out the trailing space (clipboard text)', () => {
    expect(text(formatDropPaths(['/tmp/a b.txt', '/tmp/c.txt'], { trailingSpace: false }))).toBe(
      '/tmp/a\\ b.txt /tmp/c.txt',
    )
  })
})
