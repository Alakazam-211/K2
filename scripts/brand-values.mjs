#!/usr/bin/env node
// The brand's values for the builds that are not cargo: the renderer and the
// packagers. Read from one brand file, `brand/<id>/brand.toml`, when building;
// nothing reads them at run time.
//
// The brand folder is --brand DIR, else K2_BRAND_DIR (the variable
// crates/k2-brand's build script reads), else brand/k2. A relative folder is
// read from the repo root.
//
//   node scripts/brand-values.mjs renderer [--check]
//       write src/renderer/lib/brand.generated.ts (the product's name, the
//       command's name, its aliases, the brand id); --check exits 1 when the
//       file is not what this brand makes. The committed file is brand/k2's.
//   node scripts/brand-values.mjs renderer --print
//       print that file instead of writing it.
//   node scripts/brand-values.mjs tauri [--out FILE]
//       print (or write) the Tauri config overlay: the app's name, the
//       bundle id and the update feed. `tauri build` merges it when it is in
//       TAURI_CONFIG or passed with --config. For brand/k2 it changes
//       nothing: src-tauri/tauri.conf.json holds the same values, and
//       src/renderer/lib/brand-values.test.ts fails when they drift.
//   node scripts/brand-values.mjs shell
//       print the values as shell assignments (BRAND_ID, BRAND_PRODUCT,
//       BRAND_CLI, BRAND_CLI_ALIASES, BRAND_BUNDLE_ID, BRAND_UPDATE_FEED,
//       BRAND_DESKTOP_ID, BRAND_GUI_BINARY), for a packaging script to eval.
//
// Two names, never made from each other (Rosson's rule): the product's
// ([names] product, for prose and screens) and the command's ([cli] name,
// for anything typed or copied). The app's shown name is the product's.
//
// The reader takes the brand file's own small shape (top-level keys,
// [tables], strings, integers, a list of inline tables) and refuses
// anything else, naming the file and the field, the way k2-brand does.

import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, isAbsolute, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
export const RENDERER_OUT = join(ROOT, 'src/renderer/lib/brand.generated.ts')
const DEFAULT_BRAND = 'brand/k2'
const COMMAND_NAME = /^[a-z][a-z0-9]{1,11}$/
const MODES = new Set(['silent', 'notice', 'tombstone'])
/** The keys [package] holds. All four are required when a build asks for
 *  packaging values; the renderer needs none of them. */
const PACKAGE_KEYS = ['bundle_id', 'update_feed', 'desktop_id', 'gui_binary']

/** The brand folder: the argument, else K2_BRAND_DIR, else brand/k2. */
export function brandDir(arg, env = process.env) {
  const d = arg || env.K2_BRAND_DIR || DEFAULT_BRAND
  return isAbsolute(d) ? d : join(ROOT, d)
}

function fail(file, msg) {
  throw new Error(`brand file ${relative(ROOT, file) || file}: ${msg}`)
}

/** One value: a "string", an integer, or a list of { k = "v", ... } tables
 *  (the list may span lines; the caller joined them). */
function parseValue(file, field, text) {
  const t = text.trim()
  let m = t.match(/^"([^"\\]*)"$/)
  if (m) return m[1]
  if (/^-?[0-9]+$/.test(t)) return Number(t)
  if (t.startsWith('[') && t.endsWith(']')) {
    const body = t.slice(1, -1).trim().replace(/,$/, '').trim()
    if (!body) return []
    const items = []
    const re = /\{([^{}]*)\}\s*(,|$)/gy
    let pos = 0
    re.lastIndex = 0
    while (pos < body.length) {
      re.lastIndex = pos
      const it = re.exec(body)
      if (!it) fail(file, `${field} must be a list of { name = "...", mode = "..." } tables`)
      const obj = {}
      for (const pair of it[1].split(',').map((s) => s.trim()).filter(Boolean)) {
        const kv = pair.match(/^([a-z_]+)\s*=\s*"([^"\\]*)"$/)
        if (!kv) fail(file, `${field}[${items.length}] holds something this reader does not know: ${pair}`)
        obj[kv[1]] = kv[2]
      }
      items.push(obj)
      pos = re.lastIndex
      while (pos < body.length && /\s/.test(body[pos])) pos++
    }
    return items
  }
  fail(file, `${field} is not a string, an integer or a list of tables: ${t}`)
}

/** The brand file as { top: {...}, tables: { name: {...} } }. */
function parseToml(file, text) {
  const doc = { top: {}, tables: {} }
  let cur = doc.top
  let curName = ''
  const lines = text.split('\n')
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].replace(/(^|\s)#.*$/, '').trim()
    if (!line) continue
    const tm = line.match(/^\[([a-z_]+)\]$/)
    if (tm) {
      if (doc.tables[tm[1]]) fail(file, `[${tm[1]}] appears twice`)
      cur = doc.tables[tm[1]] = {}
      curName = tm[1]
      continue
    }
    const kv = line.match(/^([a-z_]+)\s*=\s*(.*)$/)
    if (!kv) fail(file, `line ${i + 1} is not key = value: ${line}`)
    let value = kv[2]
    if (value.startsWith('[') && !value.endsWith(']')) {
      while (++i < lines.length) {
        value += ' ' + lines[i].replace(/(^|\s)#.*$/, '').trim()
        if (value.trim().endsWith(']')) break
      }
    }
    const field = curName ? `${curName}.${kv[1]}` : kv[1]
    if (kv[1] in cur) fail(file, `${field} appears twice`)
    cur[kv[1]] = parseValue(file, field, value)
  }
  return doc
}

function text(file, obj, key, field) {
  const v = obj?.[key]
  if (typeof v !== 'string') fail(file, `${field} is missing or not a string`)
  if (!v) fail(file, `${field} is empty`)
  return v
}

/** Read and check one brand folder's brand.toml. `pkg` is null when the
 *  file has no [package] table; `packageValues` refuses that. */
export function readBrand(dir) {
  const file = join(dir, 'brand.toml')
  let raw
  try {
    raw = readFileSync(file, 'utf8')
  } catch (e) {
    fail(file, `can't read it (${e.code ?? e.message}); K2_BRAND_DIR names the folder that holds brand.toml`)
  }
  const doc = parseToml(file, raw)
  if (doc.top.schema !== 1) fail(file, `schema must be 1, not ${doc.top.schema}`)
  const id = text(file, doc.top, 'id', 'id')
  if (!doc.tables.names) fail(file, '[names] is missing')
  if (!doc.tables.cli) fail(file, '[cli] is missing')
  const product = text(file, doc.tables.names, 'product', 'names.product')
  const cli = text(file, doc.tables.cli, 'name', 'cli.name')
  if (!COMMAND_NAME.test(cli)) fail(file, `cli.name "${cli}" must match ${COMMAND_NAME.source}`)
  const list = doc.tables.cli.aliases ?? []
  if (!Array.isArray(list)) fail(file, 'cli.aliases must be a list of { name, mode } tables')
  const aliases = list.map((a, i) => {
    const name = text(file, a, 'name', `cli.aliases[${i}].name`)
    const mode = text(file, a, 'mode', `cli.aliases[${i}].mode`)
    if (!COMMAND_NAME.test(name)) fail(file, `cli.aliases[${i}].name "${name}" must match ${COMMAND_NAME.source}`)
    if (!MODES.has(mode)) fail(file, `cli.aliases[${i}].mode "${mode}" must be silent, notice or tombstone`)
    if (name === cli) fail(file, `cli.aliases[${i}].name "${name}" is cli.name itself`)
    return { name, mode }
  })
  let pkg = null
  if (doc.tables.package) {
    const p = doc.tables.package
    const extra = Object.keys(p).find((k) => !PACKAGE_KEYS.includes(k))
    if (extra) fail(file, `[package] holds a key it does not know: ${extra} (it knows ${PACKAGE_KEYS.join(', ')})`)
    pkg = Object.fromEntries(PACKAGE_KEYS.map((k) => [k, text(file, p, k, `package.${k}`)]))
    if (!/^[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)+$/.test(pkg.bundle_id)) fail(file, `package.bundle_id "${pkg.bundle_id}" must be a reverse-DNS id`)
    if (!pkg.update_feed.startsWith('https://')) fail(file, `package.update_feed must be an https URL`)
    if (!/^[a-z][a-z0-9-]*$/.test(pkg.desktop_id)) fail(file, `package.desktop_id "${pkg.desktop_id}" must be a plain file name`)
    if (!/^[a-z][a-z0-9-]*$/.test(pkg.gui_binary)) fail(file, `package.gui_binary "${pkg.gui_binary}" must be a plain file name`)
  }
  return { file, id, product, cli, aliases, pkg }
}

/** The packaging values; a brand file with no [package] is refused. */
export function packageValues(brand) {
  if (!brand.pkg) fail(brand.file, `[package] is missing; a packaging build needs ${PACKAGE_KEYS.join(', ')}`)
  return brand.pkg
}

const q = (s) => `'${s.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`

/** The renderer's generated module, as text. */
export function rendererModule(brand) {
  const from = relative(ROOT, brand.file)
  return [
    `// Generated by scripts/brand-values.mjs from ${from}. Do not edit:`,
    '// change the brand file and run `node scripts/brand-values.mjs renderer`.',
    '// lib/brand.ts re-exports these; import them from there.',
    '',
    '/** The brand this build was made for. */',
    `export const BRAND_ID = ${q(brand.id)}`,
    '',
    '/** The product\'s name, as people read it: prose and screens. */',
    `export const PRODUCT_NAME = ${q(brand.product)}`,
    '',
    '/** The command people and agents type: anything typed or copied. */',
    `export const CLI_NAME = ${q(brand.cli)}`,
    '',
    '/** Other names the command answers to. */',
    `export const CLI_ALIASES: readonly string[] = [${brand.aliases.map((a) => q(a.name)).join(', ')}]`,
    '',
  ].join('\n')
}

/** The Tauri config overlay (a JSON merge patch onto tauri.conf.json). */
export function tauriOverlay(brand) {
  const p = packageValues(brand)
  return {
    productName: brand.product,
    identifier: p.bundle_id,
    plugins: { updater: { endpoints: [p.update_feed] } },
  }
}

const sh = (s) => `'${s.replace(/'/g, `'\\''`)}'`

/** The values as shell assignments. */
export function shellValues(brand) {
  const p = packageValues(brand)
  return [
    `BRAND_ID=${sh(brand.id)}`,
    `BRAND_PRODUCT=${sh(brand.product)}`,
    `BRAND_CLI=${sh(brand.cli)}`,
    `BRAND_CLI_ALIASES=${sh(brand.aliases.map((a) => a.name).join(' '))}`,
    `BRAND_BUNDLE_ID=${sh(p.bundle_id)}`,
    `BRAND_UPDATE_FEED=${sh(p.update_feed)}`,
    `BRAND_DESKTOP_ID=${sh(p.desktop_id)}`,
    `BRAND_GUI_BINARY=${sh(p.gui_binary)}`,
    '',
  ].join('\n')
}

/** Write the renderer module when it differs. Returns true when it wrote. */
export function writeRendererModule(brand, out = RENDERER_OUT) {
  const want = rendererModule(brand)
  let have = null
  try {
    have = readFileSync(out, 'utf8')
  } catch {
    have = null
  }
  if (have === want) return false
  writeFileSync(out, want)
  return true
}

/** The Vite hook: before a build or a dev server starts, write the
 *  renderer module for this build's brand (K2_BRAND_DIR, else brand/k2).
 *  vite.config.ts and vite.config.web.ts use it. */
export function viteBrandValues() {
  return {
    name: 'k2-brand-values',
    buildStart() {
      const brand = readBrand(brandDir(''))
      if (writeRendererModule(brand)) {
        console.log(`brand-values: wrote ${relative(ROOT, RENDERER_OUT)} for ${brand.id}`)
      }
    },
  }
}

function main(argv) {
  const args = argv.slice(2)
  const cmd = args.shift()
  let dirArg = ''
  let check = false
  let print = false
  let out = ''
  while (args.length) {
    const a = args.shift()
    if (a === '--brand') dirArg = args.shift() ?? ''
    else if (a === '--check') check = true
    else if (a === '--print') print = true
    else if (a === '--out') out = args.shift() ?? ''
    else throw new Error(`brand-values: unknown argument ${a}`)
  }
  const brand = readBrand(brandDir(dirArg))
  if (cmd === 'renderer') {
    const rel = relative(ROOT, RENDERER_OUT)
    if (print) return void process.stdout.write(rendererModule(brand))
    if (check) {
      let have = ''
      try {
        have = readFileSync(RENDERER_OUT, 'utf8')
      } catch {
        have = ''
      }
      if (have !== rendererModule(brand)) {
        console.error(`brand-values: ${rel} is not what ${relative(ROOT, brand.file)} makes; run node scripts/brand-values.mjs renderer`)
        process.exit(1)
      }
      console.log(`brand-values: ${rel} is current for ${brand.id}`)
      return
    }
    const wrote = writeRendererModule(brand)
    console.log(`brand-values: ${rel} ${wrote ? 'written' : 'already current'} for ${brand.id}`)
    return
  }
  if (cmd === 'tauri') {
    const json = JSON.stringify(tauriOverlay(brand), null, 2) + '\n'
    if (out) {
      const file = isAbsolute(out) ? out : join(ROOT, out)
      mkdirSync(dirname(file), { recursive: true })
      writeFileSync(file, json)
      console.log(`brand-values: wrote ${relative(ROOT, file)} for ${brand.id}`)
    } else {
      process.stdout.write(json)
    }
    return
  }
  if (cmd === 'shell') return void process.stdout.write(shellValues(brand))
  console.error('usage: brand-values.mjs renderer [--check|--print] | tauri [--out FILE] | shell   [--brand DIR]')
  process.exit(2)
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv)
  } catch (e) {
    console.error(e.message)
    process.exit(1)
  }
}
