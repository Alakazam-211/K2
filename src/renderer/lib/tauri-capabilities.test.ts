// prd-security-sweep-0464-v1 SS16 / T15: the K2 UI webviews (main,
// window-*, focus-*) don't get the generic shell, file-system or
// open-any-path powers back. This narrows what a renderer bug could reach;
// the webviews still hold K2's own commands and the owner-token daemon
// API, so a renderer XSS stays serious.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const ROOT = resolve(fileURLToPath(new URL('.', import.meta.url)), '../../..')
const read = (p: string) => readFileSync(resolve(ROOT, p), 'utf8')

type Permission = string | { identifier: string; allow?: { url?: string }[] }

describe('src-tauri capabilities (SS16)', () => {
  const cap = JSON.parse(read('src-tauri/capabilities/default.json')) as { permissions: Permission[] }
  const ids = cap.permissions.map((p) => (typeof p === 'string' ? p : p.identifier))

  it('has no shell or fs permission', () => {
    expect(ids.filter((id) => id.startsWith('shell:') || id.startsWith('fs:'))).toEqual([])
  })

  it('has no open-any-path opener permission and no file:// URL', () => {
    expect(ids).not.toContain('opener:allow-open-path')
    expect(ids).not.toContain('opener:default')
    const urls = cap.permissions.flatMap((p) => (typeof p === 'string' ? [] : (p.allow ?? []).map((a) => a.url ?? '')))
    expect(urls.length).toBeGreaterThan(0)
    expect(urls.filter((u) => /^file:/i.test(u))).toEqual([])
  })

  it('the shell and fs plugins are gone from the app and the renderer deps', () => {
    const libRs = read('src-tauri/src/lib.rs')
    expect(libRs).not.toMatch(/tauri_plugin_shell|tauri_plugin_fs/)
    const cargo = read('src-tauri/Cargo.toml')
    expect(cargo).not.toMatch(/^tauri-plugin-(shell|fs)\s*=/m)
    const pkg = JSON.parse(read('package.json')) as { dependencies: Record<string, string> }
    expect(Object.keys(pkg.dependencies).filter((d) => d === '@tauri-apps/plugin-shell' || d === '@tauri-apps/plugin-fs')).toEqual([])
  })

  it('opening a folder goes through K2’s own open_folder command', () => {
    expect(read('src-tauri/src/lib.rs')).toContain('commands::projects::open_folder,')
    expect(read('src/renderer/components/TabBar/TabBar.tsx')).not.toMatch(/\bopenPath\(/)
  })
})
