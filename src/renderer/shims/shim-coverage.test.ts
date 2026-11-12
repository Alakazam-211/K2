/**
 * The hosted web client (vite.config.web.ts) swaps every Tauri package for a
 * browser shim under src/renderer/shims/. A Tauri module the web config does
 * not alias ships the real package, which reads window.__TAURI_INTERNALS__ and
 * throws on the web (0.46.11: FileTree's getCurrentWebviewWindow crashed the
 * app). A name the renderer imports that the shim does not export is undefined
 * at run time when the import is dynamic (the build only links static ones).
 *
 * This test scans every renderer source file for Tauri imports (static and
 * dynamic), and checks that each module is aliased to a shim and each imported
 * name is a run-time export of that shim.
 */
import { describe, expect, it } from 'vitest'
import { readdirSync, readFileSync, statSync } from 'fs'
import { join, relative, resolve } from 'path'

const ROOT = resolve(__dirname, '../../..')
const RENDERER = resolve(ROOT, 'src/renderer')
const MOD = String.raw`(@tauri-apps\/[\w./-]+|@crabnebula\/[\w./-]+)`

function sourceFiles(dir: string): string[] {
  const out: string[] = []
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) {
      if (name === 'shims' || name === 'node_modules') continue
      out.push(...sourceFiles(p))
    } else if (/\.(ts|tsx)$/.test(name) && !/\.(test|mstest|wktest)\.tsx?$/.test(name)) {
      out.push(p)
    }
  }
  return out
}

/** Run-time names from a `{ a, b as c, type T, d: e }` list (types dropped). */
function names(list: string): string[] {
  return list
    .split(',')
    .map((s) => s.replace(/\/\/.*$/gm, '').trim())
    .filter((s) => s && !s.startsWith('type ') && !s.startsWith('...'))
    .map((s) => s.split(/\s+as\s+|\s*:\s*|\s*=\s*/)[0].trim())
}

type Use = { file: string; mod: string; name: string }

function scan(): { uses: Use[]; unparsed: string[] } {
  const uses: Use[] = []
  const unparsed: string[] = []
  for (const file of sourceFiles(RENDERER)) {
    const src = readFileSync(file, 'utf8')
    const rel = relative(ROOT, file)
    let parsed = 0
    const add = (mod: string, list: string[]) => {
      parsed += 1
      for (const name of list) uses.push({ file: rel, mod, name })
    }
    // import { a, b } from 'm'   (import type { … } adds no run-time names)
    for (const m of src.matchAll(new RegExp(String.raw`import\s+(type\s+)?\{([^{}]*)\}\s*from\s*['"]${MOD}['"]`, 'g'))) {
      add(m[3], m[1] ? [] : names(m[2]))
    }
    // const { a, b } = await import('m')
    for (const m of src.matchAll(new RegExp(String.raw`\{([^{}]*)\}\s*=\s*(?:await\s+)?import\(\s*['"]${MOD}['"]\s*\)`, 'g'))) {
      add(m[2], names(m[1]))
    }
    // import('m').then(({ a }) => …)
    for (const m of src.matchAll(new RegExp(String.raw`import\(\s*['"]${MOD}['"]\s*\)\s*\.then\(\s*(?:async\s*)?\(\s*\{([^{}]*)\}`, 'g'))) {
      add(m[1], names(m[2]))
    }
    // (await import('m')).a
    for (const m of src.matchAll(new RegExp(String.raw`\(\s*await\s+import\(\s*['"]${MOD}['"]\s*\)\s*\)\.(\w+)`, 'g'))) {
      add(m[1], [m[2]])
    }
    const total = [...src.matchAll(new RegExp(String.raw`(?:from\s*|import\(\s*)['"]${MOD}['"]`, 'g'))].length
    if (total !== parsed) unparsed.push(`${rel}: ${total} Tauri imports, ${parsed} understood`)
  }
  return { uses, unparsed }
}

// The web build's own config (loaded by path: it sits outside tsconfig.web.json).
const webConfig = (await import(/* @vite-ignore */ resolve(ROOT, 'vite.config.web.ts'))) as {
  default: { resolve: { alias: Record<string, string> } }
}
const aliases = webConfig.default.resolve.alias

describe('web shims cover every Tauri import in the renderer', () => {
  const { uses, unparsed } = scan()

  it('finds the imports it expects (the scan works)', () => {
    expect(unparsed).toEqual([])
    expect(uses.some((u) => u.mod === '@tauri-apps/api/core' && u.name === 'invoke')).toBe(true)
  })

  it('aliases every imported Tauri module to a shim in vite.config.web.ts', () => {
    const missing = [...new Set(uses.map((u) => u.mod))]
      .filter((mod) => !aliases[mod] || !aliases[mod].includes('/src/renderer/shims/'))
      .map((mod) => `${mod} (used in ${[...new Set(uses.filter((u) => u.mod === mod).map((u) => u.file))].join(', ')})`)
    expect(missing).toEqual([])
  })

  it('exports every imported name from the shim', async () => {
    const missing: string[] = []
    for (const mod of [...new Set(uses.map((u) => u.mod))]) {
      const shim = aliases[mod]
      if (!shim) continue // reported by the alias test
      const exports = (await import(/* @vite-ignore */ shim)) as Record<string, unknown>
      for (const u of uses.filter((x) => x.mod === mod)) {
        if (!(u.name in exports)) missing.push(`${mod}.${u.name} (${u.file})`)
      }
    }
    expect([...new Set(missing)]).toEqual([])
  })
})
