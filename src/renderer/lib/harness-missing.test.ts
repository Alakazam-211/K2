import { describe, expect, it } from 'vitest'
import { harnessMissingMessage, missingHarnessProgram } from './harness-missing'

const PTYD = 'spawn failed: v2 spawn failed: ptyd spawn: enoent: codex: not found on the child PATH'
const PORTABLE =
  "spawn failed: Unable to spawn /usr/local/bin/gemini because it doesn't exist on the filesystem and was not found in PATH"

describe('harness not installed copy (0.46.10)', () => {
  it('names the program from the k2-ptyd enoent error', () => {
    expect(missingHarnessProgram(PTYD)).toBe('codex')
    expect(harnessMissingMessage(PTYD, 'LZTEK')).toBe(
      "codex isn't installed on LZTEK. Pick another harness for this workspace, or install codex there and retry.",
    )
  })

  it('names the program from the portable-pty error (last path segment)', () => {
    expect(missingHarnessProgram(PORTABLE)).toBe('gemini')
  })

  it('says "this computer" for the local daemon', () => {
    expect(harnessMissingMessage(PTYD, 'This computer')).toContain("isn't installed on this computer.")
  })

  it('leaves every other spawn error alone', () => {
    expect(harnessMissingMessage('spawn failed: permission denied', 'LZTEK')).toBe(null)
    expect(harnessMissingMessage('', 'LZTEK')).toBe(null)
  })
})
