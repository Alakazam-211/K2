// Vendor the Zen widget standard library (prd-zen-user-widgets-v2 UWB12,
// UWB13, UWB15, UWB16). Run through `scripts/vendor-zen-lib.sh`; never in
// CI or at build time. Its output is checked in.
//
//   scripts/vendor-zen-lib.sh                 install the pinned packages, write
//                                             src/renderer/zen-lib/, zen-lib.json, NOTICE.md
//   scripts/vendor-zen-lib.sh --check         rebuild into a temp folder and fail
//                                             if anything differs from the checked-in output
//   scripts/vendor-zen-lib.sh --update-lock   re-resolve vendor-package-lock.json
//                                             (after changing a version in libs.ts and
//                                             vendor-package.json)
//
// Pinning: every package is an exact version in vendor-package.json, and
// `npm ci` checks every tarball against the sha512 integrity in
// vendor-package-lock.json. Download libraries (R8) are pinned by URL and
// by the sha256 this script records in zen-lib.json; the daemon refuses
// any other bytes.

import { createHash } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { dirname, extname, join, resolve } from 'node:path'

import { LIBS, type LibFile, type LibSpec, type StyleSource } from './libs'
import { replaceZenLibNotice } from '../../src/renderer/lib/zen/zen-lib-notice'
import { zenLibInlineProblem, zenLibInlineSafe } from '../../src/renderer/lib/zen/zen-lib-inline'
import type { ZenLibEntry, ZenLibManifest } from '../../src/renderer/lib/zen/zen-lib-loader'

const ROOT = resolve(import.meta.dir, '../..')
const HERE = join(ROOT, 'scripts/zen-lib')
const OUT = join(ROOT, 'src/renderer/zen-lib')
const MANIFEST = join(ROOT, 'src/shared/zen-lib.json')
const NOTICE = join(ROOT, 'NOTICE.md')
const LICENSE_FILE = 'LICENSE.txt'
const TARGET = ['es2020', 'safari15']

const args = new Set(process.argv.slice(2))
const CHECK = args.has('--check')
const UPDATE_LOCK = args.has('--update-lock')
const KEEP_TMP = args.has('--keep-tmp')

function die(msg: string): never {
  console.error(`vendor-zen-lib: ${msg}`)
  process.exit(1)
}

function sha256(b: Buffer | string): string {
  return createHash('sha256').update(b).digest('hex')
}

const MIME: Record<string, string> = {
  '.png': 'image/png',
  '.gif': 'image/gif',
  '.jpg': 'image/jpeg',
  '.svg': 'image/svg+xml',
  '.woff2': 'font/woff2',
  '.woff': 'font/woff',
  '.ttf': 'font/ttf',
}

// ── 1. Install the pinned packages ─────────────────────────────────────

const tmp = mkdtempSync(join(tmpdir(), 'k2-zen-lib-'))
const NM = join(tmp, 'node_modules')
const pkgJson = JSON.parse(readFileSync(join(HERE, 'vendor-package.json'), 'utf8')) as {
  dependencies: Record<string, string>
  devDependencies: Record<string, string>
}
writeFileSync(join(tmp, 'package.json'), JSON.stringify(pkgJson, null, 2))
const lockPath = join(HERE, 'vendor-package-lock.json')
const npmArgs = ['--ignore-scripts', '--no-audit', '--no-fund', '--registry=https://registry.npmjs.org/']
if (UPDATE_LOCK || !existsSync(lockPath)) {
  console.log('npm install (resolving a new lock)…')
  execFileSync('npm', ['install', ...npmArgs], { cwd: tmp, stdio: 'inherit' })
  copyFileSync(join(tmp, 'package-lock.json'), lockPath)
} else {
  copyFileSync(lockPath, join(tmp, 'package-lock.json'))
  console.log('npm ci (integrity-checked against vendor-package-lock.json)…')
  execFileSync('npm', ['ci', ...npmArgs], { cwd: tmp, stdio: 'inherit' })
}
for (const [name, want] of Object.entries({ ...pkgJson.dependencies, ...pkgJson.devDependencies })) {
  const got = JSON.parse(readFileSync(join(NM, name, 'package.json'), 'utf8')).version
  if (got !== want) die(`${name}: installed ${got}, pinned ${want}`)
}

const require = createRequire(join(tmp, 'package.json'))
const esbuild = require('esbuild') as typeof import('esbuild')
const ESBUILD_VERSION = esbuild.version

// ── 2. Build each library ──────────────────────────────────────────────

function pkgFile(path: string): string {
  const p = join(NM, path)
  if (!existsSync(p)) die(`missing package file ${path}`)
  return p
}

function dataUrl(path: string): string {
  const p = pkgFile(path)
  const mime = MIME[extname(p).toLowerCase()]
  if (!mime) die(`no MIME type for ${path}`)
  return `data:${mime};base64,${readFileSync(p).toString('base64')}`
}

/** Drop a trailing `//# sourceMappingURL=` (maps are not shipped). */
function stripSourceMap(js: string): string {
  return js.replace(/\n?\/\/# sourceMappingURL=\S+\s*$/, '\n')
}

function styleScript(src: StyleSource): string {
  let all = ''
  for (const part of src.css) {
    const p = pkgFile(part.path)
    let css = readFileSync(p, 'utf8')
    if (src.fontFaceFilter) {
      const blocks = [...css.matchAll(/\/\*\s*([^*]+?)\s*\*\/\s*@font-face\s*\{[^}]*\}/g)]
      css = blocks.filter((m) => src.fontFaceFilter!.test(m[1])).map((m) => m[0]).join('\n')
      if (!css) die(`${part.path}: the font-face filter kept nothing`)
    }
    if (src.woff2Only) {
      css = css.replace(/,\s*url\([^)]*\.(?:woff|ttf)\)\s*format\((['"])(?:woff|truetype)\1\)/g, '')
    }
    css = css.replace(/\/\*# sourceMappingURL=[^*]*\*\//g, '')
    css = css.replace(/url\((['"]?)([^'")]+)\1\)/g, (whole, _q, ref: string) => {
      if (ref.startsWith('data:') || ref.startsWith('#')) return whole
      if (/^[a-z]+:|^\/\//i.test(ref)) die(`${part.path}: remote url(${ref}) can't be inlined`)
      const file = resolve(dirname(p), ref.split(/[?#]/)[0])
      const rel = file.slice(NM.length + 1)
      return `url("${dataUrl(rel)}")`
    })
    all += (part.wrap ? part.wrap(css) : css) + '\n'
  }
  return (
    `/* K2: ${src.tag} styles, assets inlined (scripts/vendor-zen-lib.sh). */\n` +
    `(function(){var d=document,s=d.createElement("style");s.setAttribute("data-k2-lib",${JSON.stringify(src.tag)});` +
    `s.textContent=${JSON.stringify(all)};(d.head||d.documentElement).appendChild(s)})();\n`
  )
}

async function bundle(lib: LibSpec, file: { name: string; bundle: string }): Promise<string> {
  const entry = join(tmp, 'entries', `${lib.id}--${file.name}.mjs`)
  mkdirSync(dirname(entry), { recursive: true })
  writeFileSync(entry, `${file.bundle}\n`)
  const out = await esbuild.build({
    entryPoints: [entry],
    absWorkingDir: tmp,
    bundle: true,
    format: 'iife',
    globalName: lib.global,
    minify: true,
    target: TARGET,
    legalComments: 'eof',
    charset: 'utf8',
    write: false,
    metafile: true,
    logLevel: 'error',
  })
  for (const input of Object.keys(out.metafile.inputs)) {
    const m = /(?:^|\/)node_modules\/((?:@[^/]+\/)?[^/]+)\//.exec(input)
    if (m) bundledPackages.add(m[1])
  }
  const text = out.outputFiles[0].text
  return `/*! ${lib.title} ${lib.version} (${lib.license}), pre-bundled for K2 as a classic script; global ${lib.global}. */\n${text}`
}

async function fileText(lib: LibSpec, f: LibFile): Promise<string> {
  if ('copy' in f) {
    // A prebuilt UMD may carry its dependencies inside; credit the whole
    // production dependency tree (a superset of what it bundles).
    for (const dep of depTree(pkgName(f.copy))) bundledPackages.add(dep)
    return stripSourceMap(readFileSync(pkgFile(f.copy), 'utf8'))
  }
  if ('bundle' in f) return bundle(lib, f)
  if ('style' in f) return styleScript(f.style)
  return typeof f.glue === 'function' ? f.glue({ dataUrl }) : f.glue
}

async function fetchBytes(url: string): Promise<Buffer> {
  const r = await fetch(url, { redirect: 'follow' })
  if (!r.ok) die(`GET ${url}: ${r.status}`)
  return Buffer.from(await r.arrayBuffer())
}

/** Packages whose code ended up in the current library's files. Reset per
 *  library; filled by bundle() (esbuild metafile) and copies (dep tree). */
let bundledPackages = new Set<string>()

function pkgName(path: string): string {
  const parts = path.split('/')
  return path.startsWith('@') ? `${parts[0]}/${parts[1]}` : parts[0]
}

function pkgDir(name: string): string {
  const d = join(NM, name)
  if (!existsSync(join(d, 'package.json'))) die(`package ${name} is not installed`)
  return d
}

/** Production dependencies of `name`, recursively (npm installs them flat
 *  here; a nested copy is reported as missing). Types packages are skipped. */
function depTree(name: string, seen = new Set<string>()): Set<string> {
  const pj = JSON.parse(readFileSync(join(pkgDir(name), 'package.json'), 'utf8')) as { dependencies?: Record<string, string> }
  for (const dep of Object.keys(pj.dependencies ?? {})) {
    if (dep.startsWith('@types/') || dep === '@webgpu/types' || seen.has(dep)) continue
    seen.add(dep)
    depTree(dep, seen)
  }
  return seen
}

/** Licences K2 may ship inside its FSL-1.1-Apache-2.0 app. */
const ALLOWED = new Set(['MIT', 'MIT-0', 'ISC', 'BSD-2-Clause', 'BSD-3-Clause', 'Apache-2.0', '0BSD', 'Zlib', 'CC0-1.0', 'Unlicense', 'BlueOak-1.0.0', 'OFL-1.1'])

/** An SPDX expression is shippable when every AND term has an allowed OR choice. */
function licenceOk(expr: string): boolean {
  return expr
    .replace(/[()]/g, '')
    .split(/\s+AND\s+/)
    .every((term) => term.split(/\s+OR\s+/).some((id) => ALLOWED.has(id.trim())))
}

function dependencyLicences(lib: LibSpec): string {
  const own = new Set(lib.licenses.filter((l) => !l.includes(':')).map(pkgName))
  const names = [...bundledPackages].filter((n) => !own.has(n)).sort()
  if (!names.length) return ''
  const parts: string[] = []
  for (const name of names) {
    const dir = pkgDir(name)
    const pj = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')) as { version: string; license?: string; author?: unknown }
    const licence = typeof pj.license === 'string' ? pj.license : ''
    if (!licenceOk(licence)) die(`${lib.id}: bundled dependency ${name}@${pj.version} has licence "${licence || 'none'}"; flag it, don't guess`)
    const file = readdirSync(dir).find((f) => /^(licen[sc]e|copying)/i.test(f))
    const text = file
      ? readFileSync(join(dir, file), 'utf8').trimEnd()
      : `(no licence file in the npm package; package.json says ${licence}${pj.author ? `, author ${JSON.stringify(pj.author)}` : ''})`
    parts.push(`==== ${name}@${pj.version} (${licence})${file ? ` ${file}` : ''} ====\n\n${text}\n`)
  }
  return `\n\n######## Bundled dependencies (code from these packages is inside the files above) ########\n\n${parts.join('\n')}`
}

async function licenseText(lib: LibSpec): Promise<string> {
  const parts: string[] = []
  for (const l of lib.licenses) {
    let text: string
    let label = l
    if (l.startsWith('url:')) {
      label = l.slice(4)
      text = (await fetchBytes(label)).toString('utf8')
    } else if (l.startsWith('header:')) {
      label = `${l.slice(7)} (licence header)`
      const m = /^\s*(\/\*![\s\S]*?\*\/)/.exec(readFileSync(pkgFile(l.slice(7)), 'utf8'))
      if (!m) die(`${l}: no leading /*! … */ licence comment`)
      text = m[1]
    } else {
      text = readFileSync(pkgFile(l), 'utf8')
    }
    parts.push(`==== ${label} ====\n\n${text.trimEnd()}\n`)
  }
  return `${lib.title} ${lib.version} — licence texts, as shipped upstream.\n\n${parts.join('\n')}`
}

/** A script text K2 inlines into an HTML <script> element must not end it
 *  early (zen-lib-inline.ts): rewrite, then prove the rewrite worked. */
function inlineSafe(lib: LibSpec, name: string, text: string): string {
  const safe = zenLibInlineSafe(text)
  const problem = zenLibInlineProblem(safe)
  if (problem) die(`${lib.id}/${name} ${problem} after the inline rewrite`)
  return safe
}

function normalize(s: string): string {
  return s.replace(/\s+/g, ' ').trim()
}

const outRoot = CHECK ? join(tmp, 'out') : OUT
const entries: ZenLibEntry[] = []
const seen = new Set<string>()
const manifest = JSON.parse(readFileSync(MANIFEST, 'utf8')) as ZenLibManifest

for (const lib of LIBS) {
  if (lib.id === 'p5' || lib.id === 'gsap') die(`${lib.id} is not allowed (R4: p5.js is LGPL; GSAP licence)`)
  const key = `${lib.id}@${lib.version}`
  if (seen.has(key)) die(`duplicate ${key}`)
  seen.add(key)
  const dir = join(outRoot, key)
  rmSync(dir, { recursive: true, force: true })
  mkdirSync(dir, { recursive: true })

  bundledPackages = new Set<string>()
  const files: ZenLibEntry['files'] = []
  if (lib.source === 'bundled') {
    if (!lib.files.length) die(`${key}: a bundled library needs files`)
    for (const f of lib.files) {
      const text = inlineSafe(lib, f.name, await fileText(lib, f))
      const buf = Buffer.from(text, 'utf8')
      writeFileSync(join(dir, f.name), buf)
      files.push({ name: f.name, bytes: buf.length, sha256: sha256(buf) })
    }
  } else {
    if (lib.files.length || !lib.urls) die(`${key}: a download library names urls, not files`)
    for (const [name, url] of Object.entries(lib.urls)) {
      const buf = await fetchBytes(url)
      // Served as fetched (the daemon checks these exact bytes); zenLib.texts
      // applies the same inline rewrite after the check.
      if (zenLibInlineProblem(buf.toString('utf8'))) console.log(`  ${key}/${name}: rewritten for inlining at load time`)
      files.push({ name, bytes: buf.length, sha256: sha256(buf), url })
    }
  }
  if (!licenceOk(lib.license)) die(`${key}: licence "${lib.license}" is not on the shippable list; flag it, don't guess`)

  const lic = (await licenseText(lib)) + dependencyLicences(lib)
  // Each `; `-separated part (a `name: ` prefix aside) must appear in the
  // licence text, so the NOTICE line never invents an attribution.
  for (const part of lib.copyright.split('; ').map((p) => p.replace(/^[\w.-]+: /, ''))) {
    if (!normalize(lic).includes(normalize(part))) die(`${key}: "${part}" is not in its licence text`)
  }
  writeFileSync(join(dir, LICENSE_FILE), lic)
  const total = files.reduce((n, f) => n + f.bytes, 0)
  if (total > manifest.maxBytesPerWidget) die(`${key} is ${total} bytes, over the ${manifest.maxBytesPerWidget}-byte per-widget limit`)

  const bundled = lib.files.filter((f) => 'bundle' in f).map((f) => f.name)
  entries.push({
    id: lib.id,
    version: lib.version,
    title: lib.title,
    global: lib.global,
    license: lib.license,
    licenseFile: LICENSE_FILE,
    copyright: lib.copyright,
    source: lib.source,
    files,
    ...(bundled.length
      ? {
          prebundled: `esbuild ${ESBUILD_VERSION} --bundle --format=iife --global-name=${lib.global} --minify --target=${TARGET.join(',')} --legal-comments=eof (${bundled.join(', ')}; entry in scripts/zen-lib/libs.ts)`,
        }
      : {}),
    ...(lib.notes ? { notes: lib.notes } : {}),
  })
}

const next: ZenLibManifest = { ...manifest, libs: entries }
const nextJson = `${JSON.stringify(next, null, 2)}\n`

// ── 3. Write, or compare ───────────────────────────────────────────────

let problems = 0
if (CHECK) {
  if (readFileSync(MANIFEST, 'utf8') !== nextJson) {
    console.error('DIFF src/shared/zen-lib.json')
    problems++
  }
  for (const e of entries) {
    for (const f of [...e.files.filter(() => e.source === 'bundled').map((f) => f.name), LICENSE_FILE]) {
      const a = join(OUT, `${e.id}@${e.version}`, f)
      const b = join(outRoot, `${e.id}@${e.version}`, f)
      if (!existsSync(a) || sha256(readFileSync(a)) !== sha256(readFileSync(b))) {
        console.error(`DIFF src/renderer/zen-lib/${e.id}@${e.version}/${f}`)
        problems++
      }
    }
  }
  if (readFileSync(NOTICE, 'utf8') !== replaceZenLibNotice(readFileSync(NOTICE, 'utf8'), next)) {
    console.error('DIFF NOTICE.md (zen-lib section)')
    problems++
  }
} else {
  for (const d of readdirSync(OUT, { withFileTypes: true })) {
    if (d.isDirectory() && !seen.has(d.name)) {
      console.log(`removing stale ${d.name}`)
      rmSync(join(OUT, d.name), { recursive: true, force: true })
    }
  }
  writeFileSync(MANIFEST, nextJson)
  writeFileSync(NOTICE, replaceZenLibNotice(readFileSync(NOTICE, 'utf8'), next))
}

let bundledTotal = 0
for (const e of entries) {
  const n = e.files.reduce((s, f) => s + f.bytes, 0)
  if (e.source === 'bundled') bundledTotal += n
  console.log(`${`${e.id}@${e.version}`.padEnd(30)} ${e.source.padEnd(9)} ${String(n).padStart(9)} B  ${e.license}`)
}
console.log(`bundled total ${bundledTotal} B (${(bundledTotal / 1048576).toFixed(2)} MiB), esbuild ${ESBUILD_VERSION}`)
if (!KEEP_TMP) rmSync(tmp, { recursive: true, force: true })
else console.log(`kept ${tmp}`)
if (problems) die(`${problems} difference(s); run scripts/vendor-zen-lib.sh and commit the output`)
console.log(CHECK ? 'OK: checked-in output matches a fresh vendor run' : 'wrote src/renderer/zen-lib/, src/shared/zen-lib.json, NOTICE.md')
