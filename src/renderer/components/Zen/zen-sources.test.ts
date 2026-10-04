// prd-zen-mode-v1 source ratchets.
//   - T4.3 (Z20): nothing under components/Zen/ reads a Styles token. Zen's
//     look is `--zen-*` on the Zen root only.
//   - T4.7 (Z30/Z53): on macOS the native accelerator is the ONLY owner of
//     ⌃⌘Z. The one webview keydown matcher for the chord is
//     lib/zen/zen-shortcut.ts, and it installs nothing on macOS (tested in
//     zen-lib.test.ts). The menu item carries the accelerator.
//   - Z8/T4.2: Zen config never resolves the window's server.

import { describe, expect, it } from 'vitest'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const RENDERER = join(dirname(fileURLToPath(import.meta.url)), '..', '..')
const REPO = join(RENDERER, '..', '..')

function walk(dir: string, out: string[]): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) walk(p, out)
    else if (/\.(ts|tsx)$/.test(e.name) && !/\.test\.(ts|tsx)$/.test(e.name)) out.push(p)
  }
  return out
}

const rel = (p: string): string => relative(RENDERER, p).split(sep).join('/')

describe('Zen source ratchets', () => {
  it('T4.3: components/Zen never references a Styles token', () => {
    const files = walk(join(RENDERER, 'components', 'Zen'), [])
    expect(files.length).toBeGreaterThan(5)
    const hits = files.flatMap((f) => {
      const src = readFileSync(f, 'utf8')
      return ['--color-', '--radius-', '--material-', '--motion-', '--inset-window']
        .filter((t) => src.includes(t))
        .map((t) => `${rel(f)}: ${t}`)
    })
    expect(hits).toEqual([])
  })

  it('T4.7: the only webview matcher for the Zen chord is lib/zen/zen-shortcut.ts', () => {
    const owners = walk(RENDERER, [])
      .filter((f) => readFileSync(f, 'utf8').includes("'KeyZ'"))
      .map(rel)
    expect(owners).toEqual(['lib/zen/zen-shortcut.ts'])
  })

  it('T4.7: the macOS View menu item owns Ctrl+Cmd+Z and is emitted to the focused window only', () => {
    const menu = readFileSync(join(REPO, 'src-tauri', 'src', 'menu.rs'), 'utf8')
    expect(menu).toMatch(/MenuItem::with_id\(handle, "zen-toggle", zen_menu_label\(false\), true, Some\("Ctrl\+Cmd\+Z"\)\)/)
    expect(menu).toMatch(/"zen-toggle" => \{\s*emit_to_focused_window_only\(app, "menu:zen-toggle"\);/)
    // No other menu item takes the chord.
    expect(menu.match(/Some\("Ctrl\+Cmd\+Z"\)/g)?.length).toBe(1)
  })

  it('Z8: Zen code never resolves the window’s server for its config', () => {
    const files = [...walk(join(RENDERER, 'lib', 'zen'), []), ...walk(join(RENDERER, 'components', 'Zen'), [])]
    const code = (f: string): string =>
      readFileSync(f, 'utf8')
        .split('\n')
        .filter((l) => !/^\s*(\/\/|\*|\/\*)/.test(l))
        .join('\n')
    const hits = files.filter((f) => /\bprimaryScope\(/.test(code(f))).map(rel)
    expect(hits).toEqual([])
  })
})
