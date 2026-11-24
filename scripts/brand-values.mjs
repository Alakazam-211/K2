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
//       command's name, its aliases, the brand id, the company, the lockup,
//       the site, the support address and the contact address); --check
//       exits 1 when the file is
//       not what this brand makes. The committed file is brand/k2's.
//   node scripts/brand-values.mjs renderer --print
//       print that file instead of writing it.
//   node scripts/brand-values.mjs renderer --out FILE
//       write it to FILE instead (a test or a look at another brand's,
//       leaving the committed file alone).
//   node scripts/brand-values.mjs tauri [--out FILE]
//       print (or write) the Tauri config overlay: the app's name, the
//       bundle id, the update feed, the primary update key, and the
//       bundle's short description (it names the company) and homepage
//       (the site). A brand build passes the file to
//       `tauri build --config <file>`, which merges it over
//       src-tauri/tauri.conf.json. A brand with cli.no_bash true also gets
//       bundle.resources without the shell CLI (cli/k2, cli/k2so). For
//       brand/k2 it changes nothing: that
//       file already holds the same values, and
//       src/renderer/lib/brand-values.test.ts fails when they drift.
//   node scripts/brand-values.mjs shell
//       print the values as shell assignments (BRAND_ID, BRAND_PRODUCT,
//       BRAND_CLI, BRAND_CLI_ALIASES, BRAND_COMPANY, BRAND_LOCKUP,
//       BRAND_SITE, BRAND_SUPPORT_EMAIL, BRAND_BUNDLE_ID, BRAND_UPDATE_FEED,
//       BRAND_DESKTOP_ID, BRAND_GUI_BINARY, BRAND_UPDATE_PUBKEYS,
//       BRAND_NO_BASH 1 or 0), for a packaging script to eval.
//   node scripts/brand-values.mjs update-keys
//       the ship check for [update]: our brands must carry exactly K2's own
//       update keys (the ceremony's, in crates/k2-release-keys); any other
//       brand must carry none of them (nor the retired U1), by key id or by
//       key. Exits 1 and says why when not. tauri and shell refuse the same
//       way. crates/k2-release-keys/build.rs refuses it at build time too.
//
// Ours is k2 or k5 read from this repo's own brand/<id> (isOurs: the rule
// of crates/k2-brand/src/ours.rs). A copy of brand/k2 in another folder is
// not ours.
//
// Two names, never made from each other (Rosson's rule): the product's
// ([names] product, for prose and screens) and the command's ([cli] name,
// for anything typed or copied). The app's shown name is the product's.
//
// The company, the lockup, the site, the support and contact addresses and
// the source repo resolve the way k2-brand resolves them
// (crates/k2-brand/src/brand_file.rs parse_fields): names.company,
// links.site, links.support_email, links.contact_email and links.repo are
// the file's own, else the default brand file's (brand/k2); names.lockup is
// the file's own, else "<product> by <company>". A brand that isn't ours
// must set those five, and not to the default brand's value, nor
// package.update_feed (when it has [package]), the way k2-brand refuses
// them (SETS_ITS_OWN), with the same words.
//
// The reader takes the brand file's own small shape (top-level keys,
// [tables], strings, integers, true and false, a list of inline tables)
// and refuses anything else, naming the file and the field, the way
// k2-brand does.

import { existsSync, readFileSync, realpathSync, writeFileSync, mkdirSync } from 'node:fs'
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
/** The keys one [update] key holds. */
const UPDATE_KEY_KEYS = ['role', 'key_id', 'pubkey']
/** The ids of our brands, which ship K2's own update keys (the update-key
 *  ceremony's) when read from this repo's own brand/<id>
 *  (crates/k2-brand/src/ours.rs OUR_IDS). */
export const OWN_KEY_BRANDS = ['k2', 'k5']

/** The brand `id`, read from the folder `dir`, is ours: `id` is one of
 *  OWN_KEY_BRANDS and `dir` is this repo's own brand/<id>, the two compared
 *  once made canonical (crates/k2-brand/src/ours.rs is_ours). `root` is the
 *  repo's root. */
export function isOurs(id, dir, root = ROOT) {
  const mine = join(root, 'brand', id)
  if (!OWN_KEY_BRANDS.includes(id) || !existsSync(dir) || !existsSync(mine)) return false
  return realpathSync(dir) === realpathSync(mine)
}
/** Where the ceremony writes K2's own keys, and the retired key U1. */
export const RELEASE_KEYS_RS = join(ROOT, 'crates/k2-release-keys/src/lib.rs')

/** The brand folder: the argument, else K2_BRAND_DIR, else brand/k2. */
export function brandDir(arg, env = process.env) {
  const d = arg || env.K2_BRAND_DIR || DEFAULT_BRAND
  return isAbsolute(d) ? d : join(ROOT, d)
}

/** A path as people read it: from the repo root, or whole when outside it. */
function shown(file) {
  const r = relative(ROOT, file)
  return r && !r.startsWith('..') ? r : file
}

function fail(file, msg) {
  throw new Error(`brand file ${shown(file)}: ${msg}`)
}

/** One value: a "string", an integer, true or false, or a list of
 *  { k = "v", ... } tables (the list may span lines; the caller joined
 *  them). */
function parseValue(file, field, text) {
  const t = text.trim()
  let m = t.match(/^"([^"\\]*)"$/)
  if (m) return m[1]
  if (/^-?[0-9]+$/.test(t)) return Number(t)
  if (t === 'true' || t === 'false') return t === 'true'
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
      if (!it) fail(file, `${field} must be a list of { key = "...", ... } tables`)
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
  fail(file, `${field} is not a string, an integer, true or false, or a list of tables: ${t}`)
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

/** `[table] key` when the file sets it, checked by `check`; else null. */
function own(file, doc, table, key, check) {
  const t = doc.tables[table]
  if (!t || !(key in t)) return null
  const v = text(file, t, key, `${table}.${key}`)
  check(file, `${table}.${key}`, v)
  return v
}

/** The file's own value, else the default brand's; reading the default
 *  brand itself (no base), a missing one is refused. */
function inherit(file, ownValue, base, key, field) {
  if (ownValue !== null) return ownValue
  if (base) return base[key]
  fail(file, `${field} is missing; the default brand file spells every field`)
}

function checkName(file, field, v) {
  if ([...v].length > 40 || v.trim() !== v || /[\u0000-\u001f\u007f]/.test(v)) {
    fail(file, `${field} "${v}" must be one line of 1 to 40 characters with no space at either end`)
  }
}

function checkHttps(file, field, v) {
  if (!/^https:\/\/[a-z0-9.-]+\.[a-z]+(\/\S*)?$/.test(v)) fail(file, `${field} "${v}" must be an https URL: https://<domain>[/<path>]`)
}

function checkEmail(file, field, v) {
  if (!/^[A-Za-z0-9._+-]+@[a-z0-9-]+(\.[a-z0-9-]+)*\.[a-z]+$/.test(v)) fail(file, `${field} "${v}" must be an email address: <name>@<domain>`)
}

/** Read and check one brand folder's brand.toml. `pkg` is null when the
 *  file has no [package] table; `packageValues` refuses that. A field the
 *  file leaves out is the default brand's (brand/k2), read the same way. */
export function readBrand(dir) {
  const file = join(dir, 'brand.toml')
  const defaultFile = join(ROOT, DEFAULT_BRAND, 'brand.toml')
  const base = resolve(file) === resolve(defaultFile) ? null : readBrand(join(ROOT, DEFAULT_BRAND))
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
  // cli.no_bash: the build carries no shell CLI (cli/k2). Not taken from
  // the default brand: left out it is true, so only a brand that says
  // false ships the shell (brand/k2 and brand/k5 do), as k2-brand reads it.
  const noBash = doc.tables.cli.no_bash ?? true
  if (typeof noBash !== 'boolean') fail(file, 'cli.no_bash must be true or false')
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
  let update = null
  if (doc.tables.update) {
    const u = doc.tables.update
    const extra = Object.keys(u).find((k) => k !== 'keys')
    if (extra) fail(file, `[update] holds a key it does not know: ${extra} (it knows keys)`)
    if (!Array.isArray(u.keys) || u.keys.length === 0) fail(file, 'update.keys must be a list of { role, key_id, pubkey } tables, at least one')
    const keys = u.keys.map((k, i) => {
      const f = `update.keys[${i}]`
      const bad = Object.keys(k).find((x) => !UPDATE_KEY_KEYS.includes(x))
      if (bad) fail(file, `${f} holds a key it does not know: ${bad} (it knows ${UPDATE_KEY_KEYS.join(', ')})`)
      const role = text(file, k, 'role', `${f}.role`)
      const key_id = text(file, k, 'key_id', `${f}.key_id`)
      const pubkey = text(file, k, 'pubkey', `${f}.pubkey`)
      if (role !== 'primary' && role !== 'backup') fail(file, `${f}.role "${role}" must be primary or backup`)
      const id = minisignKeyId(pubkey)
      if (!id) fail(file, `${f}.pubkey is not a minisign public key line`)
      if (id !== key_id) fail(file, `${f}.key_id "${key_id}" is not the pubkey's key id (${id})`)
      return { role, key_id, pubkey }
    })
    if (keys[0].role !== 'primary') fail(file, 'update.keys must start with a primary key')
    const seen = new Set()
    for (const k of keys) {
      if (seen.has(k.key_id)) fail(file, `update.keys lists ${k.key_id} twice`)
      seen.add(k.key_id)
    }
    update = { keys }
  }
  const ours = isOurs(id, dir)
  /** A field of SETS_ITS_OWN: a brand that isn't ours sets it, and not to
   *  the default brand's value. */
  const itsOwn = (field, value, theirs) => {
    if (!base || ours) return
    if (value === null || value === undefined) fail(file, `${field} is missing: a brand that isn't ours sets its own (the default brand's would show ours)`)
    if (value === theirs) fail(file, `${field} is the default brand's own value ${JSON.stringify(value)}: a brand that isn't ours sets its own`)
  }
  const ownCompany = own(file, doc, 'names', 'company', checkName)
  itsOwn('names.company', ownCompany, base?.company)
  const company = inherit(file, ownCompany, base, 'company', 'names.company')
  const lockup = own(file, doc, 'names', 'lockup', checkName) ?? `${product} by ${company}`
  const ownSite = own(file, doc, 'links', 'site', checkHttps)
  itsOwn('links.site', ownSite, base?.site)
  const site = inherit(file, ownSite, base, 'site', 'links.site')
  const ownSupport = own(file, doc, 'links', 'support_email', checkEmail)
  itsOwn('links.support_email', ownSupport, base?.supportEmail)
  const supportEmail = inherit(file, ownSupport, base, 'supportEmail', 'links.support_email')
  const ownContact = own(file, doc, 'links', 'contact_email', checkEmail)
  itsOwn('links.contact_email', ownContact, base?.contactEmail)
  const contactEmail = inherit(file, ownContact, base, 'contactEmail', 'links.contact_email')
  const ownRepo = own(file, doc, 'links', 'repo', checkHttps)
  itsOwn('links.repo', ownRepo, base?.repo)
  const repo = inherit(file, ownRepo, base, 'repo', 'links.repo')
  if (pkg) itsOwn('package.update_feed', pkg.update_feed, base?.pkg?.update_feed)
  return { file, id, ours, product, cli, aliases, noBash, company, lockup, site, supportEmail, contactEmail, repo, pkg, update }
}

/** minisign's key id of a public key line ("Ed", 8 id bytes, 32 key bytes):
 *  the id bytes as a little-endian u64, 16 upper hex. Null when the line is
 *  not one. */
export function minisignKeyId(line) {
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(line)) return null
  const raw = Buffer.from(line, 'base64')
  if (raw.length !== 42 || raw[0] !== 0x45 || raw[1] !== 0x64) return null
  return raw.readBigUInt64LE(2).toString(16).toUpperCase().padStart(16, '0')
}

/** Tauri's wrapped form of a key line (plugins.updater.pubkey). */
export function tauriPubkey(line) {
  return Buffer.from(`untrusted comment: minisign public key: ${minisignKeyId(line)}\n${line}\n`).toString('base64')
}

/** K2's own update keys, as the ceremony wrote them in k2-release-keys
 *  (UPDATER_KEYS, in order, with roles), and the retired key U1. */
export function ownUpdateKeys(file = RELEASE_KEYS_RS) {
  const src = readFileSync(file, 'utf8')
  const a = src.indexOf('// BEGIN UPDATER_KEYS')
  const b = src.indexOf('// END UPDATER_KEYS')
  if (a < 0 || b < a) throw new Error(`${relative(ROOT, file)}: no UPDATER_KEYS block`)
  const keys = [...src.slice(a, b).matchAll(/role: "([a-z]+)",\s*key_id: "([0-9A-F]{16})",\s*public_key_b64: "([^"]+)"/g)].map((m) => ({
    role: m[1],
    key_id: m[2],
    pubkey: m[3],
  }))
  const legacy = src.match(/^pub const LEGACY_UPDATER_KEY_B64: &str = "([^"]+)";$/m)
  const legacyId = src.match(/^pub const LEGACY_UPDATER_KEY_ID: &str = "([0-9A-F]{16})";$/m)
  if (!legacy || !legacyId) throw new Error(`${relative(ROOT, file)}: no LEGACY_UPDATER_KEY_B64 / _ID`)
  return { keys, retired: { key_id: legacyId[1], pubkey: legacy[1] } }
}

/** What is wrong with a brand's update keys, as lines (empty: nothing).
 *  Our brands (brand.ours: isOurs) must carry exactly K2's own keys; any
 *  other brand must carry none of them, nor the retired one, by key id or
 *  by key. */
export function updateKeyProblems(brand, own = ownUpdateKeys()) {
  if (!brand.update) return [`${shown(brand.file)}: [update] is missing; a build needs its own update keys`]
  const mine = brand.update.keys
  if (brand.ours) {
    const want = own.keys.map((k) => `${k.role} ${k.key_id} ${k.pubkey}`).join('\n')
    const have = mine.map((k) => `${k.role} ${k.key_id} ${k.pubkey}`).join('\n')
    return want === have ? [] : [`${shown(brand.file)}: brand ${brand.id} must carry exactly the ceremony's update keys (crates/k2-release-keys UPDATER_KEYS), in order`]
  }
  const ours = [...own.keys, own.retired]
  const out = []
  for (const k of mine) {
    for (const o of ours) {
      if (k.key_id === o.key_id || k.pubkey === o.pubkey) {
        out.push(`${shown(brand.file)}: brand ${brand.id} carries the ceremony's update key ${o.key_id}; only ${OWN_KEY_BRANDS.join(' and ')} may ship it`)
      }
    }
  }
  return out
}

/** The brand's update keys, refused when updateKeyProblems finds any. */
export function updateKeys(brand) {
  const problems = updateKeyProblems(brand)
  if (problems.length) throw new Error(problems.join('\n'))
  return brand.update.keys
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
    '// Typed as string, not as the literal, so code type-checks the same for',
    '// every brand.',
    '',
    '/** The brand this build was made for. */',
    `export const BRAND_ID: string = ${q(brand.id)}`,
    '',
    '/** The product\'s name, as people read it: prose and screens. */',
    `export const PRODUCT_NAME: string = ${q(brand.product)}`,
    '',
    '/** The command people and agents type: anything typed or copied. */',
    `export const CLI_NAME: string = ${q(brand.cli)}`,
    '',
    '/** Other names the command answers to. */',
    `export const CLI_ALIASES: readonly string[] = [${brand.aliases.map((a) => q(a.name)).join(', ')}]`,
    '',
    '/** The company that makes it. */',
    `export const COMPANY_NAME: string = ${q(brand.company)}`,
    '',
    '/** The product with its maker. */',
    `export const PRODUCT_LOCKUP: string = ${q(brand.lockup)}`,
    '',
    '/** The public website. */',
    `export const PUBLIC_SITE: string = ${q(brand.site)}`,
    '',
    '/** Where people write for help. */',
    `export const SUPPORT_EMAIL: string = ${q(brand.supportEmail)}`,
    '',
    '/** Where people write when their server can\'t send feedback. */',
    `export const CONTACT_EMAIL: string = ${q(brand.contactEmail)}`,
    '',
  ].join('\n')
}

/** The one line that says what the app is and who makes it: the bundle's
 *  short description, the Linux package's and the .desktop file's. */
export function shortDescription(brand) {
  return `AI workspace orchestration by ${brand.company}`
}

/** The shell CLI's files in the app's resources: the script and its alias
 *  shim. A build with cli.no_bash ships neither. */
export const SHELL_CLI_RESOURCES = ['../cli/k2', '../cli/k2so']
export const TAURI_CONF = join(ROOT, 'src-tauri/tauri.conf.json')

/** The app's resources without the shell CLI's files, from `conf` (the
 *  base tauri.conf.json). Each must be there: a rename stops the build
 *  rather than ship a script by accident. */
export function resourcesWithoutShell(conf) {
  const list = conf?.bundle?.resources
  if (!Array.isArray(list)) throw new Error(`${shown(TAURI_CONF)}: bundle.resources is not a list`)
  for (const r of SHELL_CLI_RESOURCES) {
    if (!list.includes(r)) throw new Error(`${shown(TAURI_CONF)}: bundle.resources has no ${r}; update SHELL_CLI_RESOURCES`)
  }
  return list.filter((r) => !SHELL_CLI_RESOURCES.includes(r))
}

/** The Tauri config overlay (a JSON merge patch onto tauri.conf.json). A
 *  merge patch replaces a list whole, so a no-bash brand's overlay restates
 *  the base's resources less the shell CLI's files, beside the bundle's
 *  other keys. */
export function tauriOverlay(brand, conf = null) {
  const keys = updateKeys(brand)
  const p = packageValues(brand)
  const overlay = {
    productName: brand.product,
    identifier: p.bundle_id,
    bundle: { shortDescription: shortDescription(brand), homepage: brand.site },
    plugins: { updater: { endpoints: [p.update_feed], pubkey: tauriPubkey(keys[0].pubkey) } },
  }
  if (brand.noBash) {
    const base = conf ?? JSON.parse(readFileSync(TAURI_CONF, 'utf8'))
    overlay.bundle.resources = resourcesWithoutShell(base)
  }
  return overlay
}

const sh = (s) => `'${s.replace(/'/g, `'\\''`)}'`

/** The values as shell assignments. */
export function shellValues(brand) {
  const keys = updateKeys(brand)
  const p = packageValues(brand)
  return [
    `BRAND_ID=${sh(brand.id)}`,
    `BRAND_PRODUCT=${sh(brand.product)}`,
    `BRAND_CLI=${sh(brand.cli)}`,
    `BRAND_CLI_ALIASES=${sh(brand.aliases.map((a) => a.name).join(' '))}`,
    `BRAND_COMPANY=${sh(brand.company)}`,
    `BRAND_LOCKUP=${sh(brand.lockup)}`,
    `BRAND_SITE=${sh(brand.site)}`,
    `BRAND_SUPPORT_EMAIL=${sh(brand.supportEmail)}`,
    `BRAND_BUNDLE_ID=${sh(p.bundle_id)}`,
    `BRAND_UPDATE_FEED=${sh(p.update_feed)}`,
    `BRAND_DESKTOP_ID=${sh(p.desktop_id)}`,
    `BRAND_GUI_BINARY=${sh(p.gui_binary)}`,
    `BRAND_UPDATE_PUBKEYS=${sh(keys.map((k) => k.pubkey).join(' '))}`,
    `BRAND_NO_BASH=${sh(brand.noBash ? '1' : '0')}`,
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
    if (out) {
      if (check) throw new Error('brand-values: --check reads the committed file; leave --out out')
      const file = isAbsolute(out) ? out : join(ROOT, out)
      mkdirSync(dirname(file), { recursive: true })
      writeRendererModule(brand, file)
      console.log(`brand-values: wrote ${shown(file)} for ${brand.id}`)
      return
    }
    if (check) {
      let have = ''
      try {
        have = readFileSync(RENDERER_OUT, 'utf8')
      } catch {
        have = ''
      }
      if (have !== rendererModule(brand)) {
        console.error(`brand-values: ${rel} is not what ${shown(brand.file)} makes; run node scripts/brand-values.mjs renderer`)
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
  if (cmd === 'update-keys') {
    const problems = updateKeyProblems(brand)
    if (problems.length) {
      for (const line of problems) console.error(`brand-values: REFUSED ${line}`)
      process.exit(1)
    }
    for (const k of brand.update.keys) console.log(`${k.role} ${k.key_id} ${k.pubkey}`)
    console.log(`brand-values: ${brand.id}'s update keys are its own`)
    return
  }
  console.error('usage: brand-values.mjs renderer [--check|--print|--out FILE] | tauri [--out FILE] | shell | update-keys   [--brand DIR]')
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
