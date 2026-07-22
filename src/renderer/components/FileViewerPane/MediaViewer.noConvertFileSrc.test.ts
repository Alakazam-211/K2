import { describe, it, expect } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

// Strip line and block comments so source docstrings can mention banned
// APIs without false-positiving the guardrail.
function stripComments(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:])\/\/.*$/gm, '$1')
}

/**
 * Guardrail for R4/R5: MediaViewer must play on the viewer client via
 * Blob URLs from daemon binary reads — never convertFileSrc (host-local
 * only; breaks remote + web).
 */
describe('MediaViewer — no convertFileSrc', () => {
  it('does not import or call convertFileSrc', () => {
    const raw = readFileSync(resolve(__dirname, 'MediaViewer.tsx'), 'utf8')
    const src = stripComments(raw)
    expect(src).not.toMatch(/convertFileSrc/)
    expect(src).not.toMatch(/@tauri-apps\/api/)
    expect(src).toMatch(/loadHostBinary/)
    expect(src).toMatch(/bytesToObjectUrl|createObjectURL/)
    expect(raw).toMatch(/Playing on this device/)
    expect(src).toMatch(/<audio/)
    expect(src).toMatch(/<video/)
  })

  it('load-host-binary helper does not use convertFileSrc', () => {
    const raw = readFileSync(
      resolve(__dirname, '../../lib/load-host-binary.ts'),
      'utf8',
    )
    const src = stripComments(raw)
    expect(src).not.toMatch(/convertFileSrc/)
    expect(src).toMatch(/fs\/read-binary/)
    expect(src).toMatch(/fs\/read-range/)
  })
})
