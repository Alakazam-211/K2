// The code viewer's shiki allowlist gate. It fails when:
//   - a grammar or theme id outside the allowlist can be loaded through the
//     renderer's shiki entry (lib/shiki-allowlist.ts), or any renderer file
//     imports shiki's grammars or themes some other way;
//   - the allowlist holds any of the hard-excluded ids below, whatever the
//     left-out list says;
//   - the generated allowlist and the left-out list disagree (rerun
//     `node scripts/gen-shiki-allowlist.mjs`);
//   - a kept grammar embeds a left-out one the build plugin would not stub.

import { createHash } from 'crypto'
import { readdirSync, readFileSync, statSync } from 'fs'
import { createRequire } from 'module'
import { dirname, join, relative, resolve } from 'path'
import { describe, expect, it } from 'vitest'
import { bundledLanguagesInfo } from 'shiki/langs'
import { bundledThemesInfo } from 'shiki/themes'
import {
  SHIKI_EXCLUDED,
  SHIKI_EXCLUDED_TEXT,
  SHIKI_LANG_IDS,
  SHIKI_THEME_IDS,
  isShikiLangAllowed,
  loadAllowedShikiLang,
  resolveShikiLang,
  shikiLangLoader,
  shikiThemeLoader,
} from './shiki-allowlist'
import { GENERATED_LANG_ALIASES, GENERATED_LANG_LOADERS, GENERATED_THEME_LOADERS } from './shiki-allowlist.generated'
import { SHIKI_EXCLUDED_FILE, leftOutShikiModule, readShikiExcluded } from './shiki-vite-plugin'

const REPO = resolve(__dirname, '../../..')
const RENDERER = resolve(REPO, 'src/renderer')

// Out for good, whatever the left-out list says.
const DROPPED_LANGS = ['ada', 'gnuplot', 'nginx', 'racket']
const DROPPED_THEMES = ['aurora-x']
// Accepted by the owner with their licence texts (NOTICE.md, resources/notices/): on the allowlist.
const ACCEPTED_LANGS = ['abap', 'applescript', 'erb', 'logo', 'mipsasm', 'po', 'ssh-config', 'toml', 'bird2', 'hcl', 'terraform']

const ALL_LANGS = bundledLanguagesInfo.map((l) => l.id)
const ALL_THEMES = bundledThemesInfo.map((t) => t.id)
const OUT_LANGS = ALL_LANGS.filter((id) => !SHIKI_LANG_IDS.includes(id))
const OUT_THEMES = ALL_THEMES.filter((id) => !SHIKI_THEME_IDS.includes(id))

describe('shiki allowlist: the left-out list', () => {
  it('the renderer and the build plugin read the same file', () => {
    expect(SHIKI_EXCLUDED_TEXT).toBe(readFileSync(resolve(REPO, SHIKI_EXCLUDED_FILE), 'utf8'))
    const fromPlugin = readShikiExcluded(REPO)
    expect([...fromPlugin.langs].sort()).toEqual([...SHIKI_EXCLUDED.langs].sort())
    expect([...fromPlugin.themes].sort()).toEqual([...SHIKI_EXCLUDED.themes].sort())
  })

  it('lists the dropped grammars and theme (so the build plugin stubs them)', () => {
    for (const id of DROPPED_LANGS) expect(SHIKI_EXCLUDED.langs.has(id), `grammar ${id}`).toBe(true)
    for (const id of DROPPED_THEMES) expect(SHIKI_EXCLUDED.themes.has(id), `theme ${id}`).toBe(true)
  })

  it('names only ids shiki has', () => {
    for (const id of SHIKI_EXCLUDED.langs) expect(ALL_LANGS, `grammar ${id}`).toContain(id)
    for (const id of SHIKI_EXCLUDED.themes) expect(ALL_THEMES, `theme ${id}`).toContain(id)
  })
})

describe('shiki allowlist: hard-excluded ids', () => {
  it('never holds the dropped grammars and theme', () => {
    for (const id of DROPPED_LANGS) {
      expect(SHIKI_LANG_IDS, `grammar ${id}`).not.toContain(id)
      expect(Object.keys(GENERATED_LANG_LOADERS), `generated grammar ${id}`).not.toContain(id)
      expect(Object.values(GENERATED_LANG_ALIASES), `alias target ${id}`).not.toContain(id)
      expect(isShikiLangAllowed(id), id).toBe(false)
    }
    for (const id of DROPPED_THEMES) {
      expect(SHIKI_THEME_IDS, `theme ${id}`).not.toContain(id)
      expect(Object.keys(GENERATED_THEME_LOADERS), `generated theme ${id}`).not.toContain(id)
    }
  })

  it('holds the accepted grammars, each with its loader', () => {
    for (const id of ACCEPTED_LANGS) {
      expect(SHIKI_EXCLUDED.langs.has(id), `left out: ${id}`).toBe(false)
      expect(isShikiLangAllowed(id), id).toBe(true)
      expect(shikiLangLoader(id), `grammar ${id}`).toBeDefined()
    }
    for (const a of ['tf', 'tfvars', 'TF', 'Terraform', 'hcl']) expect(resolveShikiLang(a), `alias ${a}`).toBeDefined()
  })
})

describe('shiki allowlist: the accepted grammars ship their licence texts', () => {
  const notice = readFileSync(resolve(REPO, 'NOTICE.md'), 'utf8')
  // bundle.resources: a list of paths, or a map of path to where it lands in the app.
  const conf = JSON.parse(readFileSync(resolve(REPO, 'src-tauri/tauri.conf.json'), 'utf8')) as { bundle: { resources: string[] | Record<string, string> } }
  const shipped = Array.isArray(conf.bundle.resources) ? conf.bundle.resources : Object.keys(conf.bundle.resources)

  it.each(ACCEPTED_LANGS)('%s: a NOTICE.md row, its text in resources/notices/ with that sha256, shipped in the app', (id) => {
    const rows = notice.split('\n').filter((l) => l.startsWith(`| \`${id}\` |`))
    expect(rows, `NOTICE.md rows for ${id}`).toHaveLength(1)
    const m = /\| `(shiki-[a-z0-9-]+-LICENSE)` \| `([0-9a-f]{64})` \|$/.exec(rows[0])
    expect(m, rows[0]).not.toBeNull()
    const [, file, sha] = m!
    const text = readFileSync(resolve(REPO, 'resources/notices', file))
    expect(createHash('sha256').update(text).digest('hex'), file).toBe(sha)
    expect(shipped, file).toContain(`../resources/notices/${file}`)
  })
})

describe('shiki allowlist: generated file matches the list', () => {
  it('generated ids are exactly shiki minus the left-out list', () => {
    expect(Object.keys(GENERATED_LANG_LOADERS).sort()).toEqual(ALL_LANGS.filter((id) => !SHIKI_EXCLUDED.langs.has(id)).sort())
    expect(Object.keys(GENERATED_THEME_LOADERS).sort()).toEqual(ALL_THEMES.filter((id) => !SHIKI_EXCLUDED.themes.has(id)).sort())
    expect([...SHIKI_LANG_IDS].sort()).toEqual(Object.keys(GENERATED_LANG_LOADERS).sort())
    expect([...SHIKI_THEME_IDS].sort()).toEqual(Object.keys(GENERATED_THEME_LOADERS).sort())
  })

  it('every generated loader imports its own id, and no left-out id appears', () => {
    const src = readFileSync(resolve(RENDERER, 'lib/shiki-allowlist.generated.ts'), 'utf8')
    const langImports = [...src.matchAll(/import\('shiki\/langs\/([^']+)\.mjs'\)/g)].map((m) => m[1])
    const themeImports = [...src.matchAll(/import\('shiki\/themes\/([^']+)\.mjs'\)/g)].map((m) => m[1])
    expect(langImports.sort()).toEqual([...SHIKI_LANG_IDS].sort())
    expect(themeImports.sort()).toEqual([...SHIKI_THEME_IDS].sort())
  })

  it('keeps the theme the code viewer uses', () => {
    expect(SHIKI_THEME_IDS).toContain('github-dark-default')
  })
})

describe('shiki allowlist: nothing outside it is importable', () => {
  it('every grammar and theme outside the allowlist has no loader and resolves to nothing', async () => {
    expect(OUT_LANGS.length).toBeGreaterThan(0)
    for (const id of OUT_LANGS) {
      expect(shikiLangLoader(id), `grammar ${id}`).toBeUndefined()
      expect(resolveShikiLang(id), `grammar ${id}`).toBeUndefined()
      expect(await loadAllowedShikiLang(id), `grammar ${id}`).toBeUndefined()
    }
    for (const id of OUT_THEMES) expect(shikiThemeLoader(id), `theme ${id}`).toBeUndefined()
  })

  it('aliases and other spellings of left-out grammars resolve to nothing', () => {
    const outAliases = bundledLanguagesInfo.filter((l) => OUT_LANGS.includes(l.id)).flatMap((l) => l.aliases ?? [])
    for (const a of outAliases) expect(resolveShikiLang(a), `alias ${a}`).toBeUndefined()
    for (const a of ['NGINX', 'Nginx', ' nginx ', 'Racket', 'ADA', 'GnuPlot', 'conf']) expect(resolveShikiLang(a), `alias ${a}`).toBeUndefined()
  })

  it('a loaded grammar carries no left-out registration or embedded name', async () => {
    for (const id of ['cpp', 'ruby', 'rst', 'handlebars', 'just', 'mdc', 'elm', 'julia', 'twig']) {
      const regs = await loadAllowedShikiLang(id)
      expect(regs, id).toBeDefined()
      for (const r of regs!) {
        expect(SHIKI_EXCLUDED.langs.has(r.name), `${id} carries ${r.name}`).toBe(false)
        for (const e of [...(r.embeddedLangs ?? []), ...(r.embeddedLangsLazy ?? [])]) {
          expect(SHIKI_EXCLUDED.langs.has(e), `${id}/${r.name} embeds ${e}`).toBe(false)
        }
      }
    }
  })

  it('no renderer file reaches shiki grammars or themes except the allowlist', () => {
    const files: string[] = []
    const walk = (dir: string) => {
      for (const name of readdirSync(dir)) {
        if (name === 'node_modules') continue
        const p = join(dir, name)
        if (statSync(p).isDirectory()) walk(p)
        else if (/\.(ts|tsx|js|jsx|mjs)$/.test(name) && !/\.test\.(ts|tsx)$/.test(name)) files.push(p)
      }
    }
    walk(RENDERER)
    const spec = /(?:from\s*|import\s*\(\s*|require\s*\(\s*)['"]((?:shiki|@shikijs\/[^'"/]+)(?:\/[^'"]*)?)['"]/g
    const allowedAnywhere = new Set(['shiki/core', 'shiki/engine/oniguruma', 'shiki/engine/javascript', 'shiki/wasm'])
    const bad: string[] = []
    for (const f of files) {
      const rel = relative(REPO, f)
      for (const m of readFileSync(f, 'utf8').matchAll(spec)) {
        const s = m[1]
        if (allowedAnywhere.has(s)) continue
        if (rel === 'src/renderer/lib/shiki-allowlist.generated.ts' && /^shiki\/(langs|themes)\/[^/]+\.mjs$/.test(s)) continue
        bad.push(`${rel}: ${s}`)
      }
    }
    expect(bad).toEqual([])
  })
})

describe('shiki allowlist: build plugin', () => {
  const require = createRequire(resolve(REPO, 'package.json'))
  const shikiDir = dirname(require.resolve('shiki/package.json'))
  // The grammar files themselves (where one grammar imports another).
  const langsDist = dirname(createRequire(join(shikiDir, 'package.json')).resolve('@shikijs/langs/cpp'))

  it('stubs left-out modules on every path and leaves kept ones alone', () => {
    const fromCpp = join(langsDist, 'cpp.mjs')
    expect(leftOutShikiModule(SHIKI_EXCLUDED, './glsl.mjs', fromCpp)).toEqual({ kind: 'lang', id: 'glsl' })
    expect(leftOutShikiModule(SHIKI_EXCLUDED, './glsl.mjs', join(langsDist, 'elm.mjs'))).toEqual({ kind: 'lang', id: 'glsl' })
    // yaml is allowed: ruby's embedded yaml is a real grammar, not a stub.
    expect(leftOutShikiModule(SHIKI_EXCLUDED, './yaml.mjs', join(langsDist, 'ruby.mjs'))).toBeUndefined()
    expect(leftOutShikiModule(SHIKI_EXCLUDED, 'shiki/langs/nginx.mjs', '/x/src/a.ts')).toEqual({ kind: 'lang', id: 'nginx' })
    expect(leftOutShikiModule(SHIKI_EXCLUDED, '@shikijs/langs/racket', '/x/a.ts')).toEqual({ kind: 'lang', id: 'racket' })
    expect(leftOutShikiModule(SHIKI_EXCLUDED, '@shikijs/langs/terraform', '/x/a.ts')).toBeUndefined()
    expect(leftOutShikiModule(SHIKI_EXCLUDED, '@shikijs/themes/aurora-x', '/x/a.ts')).toEqual({ kind: 'theme', id: 'aurora-x' })
    expect(leftOutShikiModule(SHIKI_EXCLUDED, './c.mjs', fromCpp)).toBeUndefined()
    expect(leftOutShikiModule(SHIKI_EXCLUDED, 'shiki/langs/cpp.mjs', '/x/a.ts')).toBeUndefined()
    expect(leftOutShikiModule(SHIKI_EXCLUDED, './glsl.mjs', '/x/src/renderer/a.ts')).toBeUndefined()
  })

  it('every left-out grammar a kept grammar reaches is stubbed', () => {
    const imports = (id: string): string[] =>
      [...readFileSync(join(langsDist, `${id}.mjs`), 'utf8').matchAll(/from '\.\/([^']+)\.mjs'/g)].map((m) => m[1])
    const reached = new Set<string>()
    const seen = new Set<string>()
    const visit = (id: string) => {
      if (seen.has(id)) return
      seen.add(id)
      for (const dep of imports(id)) {
        const hit = leftOutShikiModule(SHIKI_EXCLUDED, `./${dep}.mjs`, join(langsDist, `${id}.mjs`))
        if (hit) { reached.add(dep); continue }
        expect(SHIKI_EXCLUDED.langs.has(dep), `${id} -> ${dep}`).toBe(false)
        visit(dep)
      }
    }
    for (const id of SHIKI_LANG_IDS) visit(id)
    // cpp/elm/julia/es-tag-glsl embed glsl.
    expect([...reached]).toContain('glsl')
    for (const id of reached) expect(SHIKI_EXCLUDED.langs.has(id), id).toBe(true)
  })
})
