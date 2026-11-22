// scripts/brand-values.mjs: the brand's values for the renderer and the
// packagers, read from the brand file when building. These tests run the
// script the way a build does and read what it prints.

import { execFileSync, spawnSync } from 'node:child_process'
import { generateKeyPairSync, randomBytes } from 'node:crypto'
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync, mkdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { afterAll, describe, expect, it } from 'vitest'
import { BRAND_ID, CLI_ALIASES, CLI_NAME, PRODUCT_NAME, RELEASE_REPO } from './brand'

const ROOT = resolve(__dirname, '../../..')
const SCRIPT = join(ROOT, 'scripts/brand-values.mjs')
const scratch = mkdtempSync(join(tmpdir(), 'brand-values-'))
afterAll(() => rmSync(scratch, { recursive: true, force: true }))

/** Run the script with a clean K2_BRAND_DIR; throws on a non-zero exit. */
function run(args: string[], env: Record<string, string> = {}): string {
  const { K2_BRAND_DIR: _drop, ...rest } = process.env
  return execFileSync(process.execPath, [SCRIPT, ...args], { cwd: ROOT, encoding: 'utf8', env: { ...rest, ...env } })
}

/** Run it expecting a refusal; returns stderr. */
function refused(args: string[]): string {
  const { K2_BRAND_DIR: _drop, ...rest } = process.env
  const r = spawnSync(process.execPath, [SCRIPT, ...args], { cwd: ROOT, encoding: 'utf8', env: rest })
  expect(r.status, r.stdout + r.stderr).toBe(1)
  return r.stderr
}

/** `key = "value"` from one [table] of a brand file, read on its own. */
function field(file: string, table: string, key: string): string {
  const text = readFileSync(file, 'utf8')
  const body = text.split(/^\[/m).find((part) => part.startsWith(`${table}]`))
  if (!body) throw new Error(`no [${table}] in ${file}`)
  const m = body.match(new RegExp(`^${key} = "([^"]*)"$`, 'm'))
  if (!m) throw new Error(`no ${table}.${key} in ${file}`)
  return m[1]
}

/** A brand folder in the scratch dir holding `text` as brand.toml. */
function brandFolder(name: string, text: string): string {
  const dir = join(scratch, name)
  mkdirSync(dir, { recursive: true })
  writeFileSync(join(dir, 'brand.toml'), text)
  return dir
}

/** A minisign public key line made for this run; its private half is
 *  dropped at once, so nothing can sign for it. */
function throwawayKey(): { id: string; line: string } {
  const { publicKey } = generateKeyPairSync('ed25519')
  const x = Buffer.from(publicKey.export({ format: 'jwk' }).x as string, 'base64url')
  const id = randomBytes(8)
  return { id: id.readBigUInt64LE().toString(16).toUpperCase().padStart(16, '0'), line: Buffer.concat([Buffer.from('Ed'), id, x]).toString('base64') }
}
const TEST_KEY = throwawayKey()

/** K2's own keys, as the brand file spells them (brand/k2's [update]). */
const K2_PRIMARY = { id: '76CA14D3B7EE2B69', line: 'RWRpK+630xTKdopKvlpWVqvnEwqyK1CZ+F5deeRjpGA2ywyWS8NnOrpv' }
const K2_RETIRED = { id: 'E9111446F65EBB05', line: 'RWQFu172RhQR6QBqzmij2E9btDGhDWmpdH+ah1/M4PmuHPINUWvKLf6o' }

/** An [update] table holding one primary key. */
const updateTable = (k: { id: string; line: string }) => `[update]\nkeys = [\n    { role = "primary", key_id = "${k.id}", pubkey = "${k.line}" },\n]\n`

/** brand/xy's own file with its [update] table swapped for `k`. */
function xyWithKey(name: string, k: { id: string; line: string }): string {
  const xy = readFileSync(join(ROOT, 'brand/xy/brand.toml'), 'utf8')
  const at = xy.indexOf('\n[update]\n')
  if (at < 0) throw new Error('brand/xy has no [update] table')
  const rest = xy.slice(at + 1)
  const end = rest.indexOf('\n]\n')
  if (end < 0) throw new Error('brand/xy [update] keys list does not end with ]')
  return brandFolder(name, xy.slice(0, at + 1) + updateTable(k) + rest.slice(end + 3))
}

const GOOD = [
  'id = "ab"',
  'schema = 1',
  '[names]',
  'product = "Acme Desk"',
  '[cli]',
  'name = "ab"',
  'aliases = [',
  '    { name = "cd", mode = "notice" },',
  ']',
  '[package]',
  'bundle_id = "com.example.acme"',
  'update_feed = "https://updates.example/acme/latest.json"',
  'desktop_id = "acme"',
  'gui_binary = "acme-gui"',
  '',
].join('\n').replace('[package]', `${updateTable(TEST_KEY)}[package]`)

describe('the renderer module', () => {
  it('is what brand/k2 makes (run `node scripts/brand-values.mjs renderer` when this fails)', () => {
    expect(run(['renderer', '--check'])).toContain('is current for k2')
  })

  it('carries the default brand file\'s names, each from its own field', () => {
    const file = join(ROOT, 'brand/k2/brand.toml')
    expect(BRAND_ID).toBe('k2')
    expect(PRODUCT_NAME).toBe(field(file, 'names', 'product'))
    expect(CLI_NAME).toBe(field(file, 'cli', 'name'))
    expect(CLI_ALIASES).toEqual(['k2so'])
  })

  it('every brand in the repo makes a module with its own two names', () => {
    for (const id of readdirSync(join(ROOT, 'brand'), { withFileTypes: true }).filter((d) => d.isDirectory()).map((d) => d.name)) {
      const file = join(ROOT, 'brand', id, 'brand.toml')
      const out = run(['renderer', '--print', '--brand', `brand/${id}`])
      expect(out, id).toContain(`export const BRAND_ID: string = '${id}'`)
      expect(out, id).toContain(`export const PRODUCT_NAME: string = '${field(file, 'names', 'product')}'`)
      expect(out, id).toContain(`export const CLI_NAME: string = '${field(file, 'cli', 'name')}'`)
    }
  })

  it('K2_BRAND_DIR picks the brand, as it does for cargo', () => {
    const dir = brandFolder('env', GOOD)
    const out = run(['renderer', '--print'], { K2_BRAND_DIR: dir })
    expect(out).toContain("export const PRODUCT_NAME: string = 'Acme Desk'")
    expect(out).toContain("export const CLI_NAME: string = 'ab'")
    expect(out).toContain("export const CLI_ALIASES: readonly string[] = ['cd']")
  })

  it('a product name with a quote is written as a string', () => {
    const dir = brandFolder('quote', GOOD.replace('"Acme Desk"', '"Acme\'s Desk"'))
    expect(run(['renderer', '--print', '--brand', dir])).toContain("export const PRODUCT_NAME: string = 'Acme\\'s Desk'")
  })
})

describe('the packaging values', () => {
  it('brand/k2 is what src-tauri/tauri.conf.json already says, so the default build is unchanged', () => {
    const conf = JSON.parse(readFileSync(join(ROOT, 'src-tauri/tauri.conf.json'), 'utf8'))
    const overlay = JSON.parse(run(['tauri']))
    expect(overlay).toEqual({
      productName: conf.productName,
      identifier: conf.identifier,
      plugins: { updater: { endpoints: conf.plugins.updater.endpoints, pubkey: conf.plugins.updater.pubkey } },
    })
  })

  it('brand/k2 is what the Arch package installs (command, desktop file, GUI program)', () => {
    const values = Object.fromEntries(
      run(['shell'])
        .trim()
        .split('\n')
        .map((l) => {
          const m = l.match(/^([A-Z_]+)='(.*)'$/)
          if (!m) throw new Error(`not an assignment: ${l}`)
          return [m[1], m[2]]
        }),
    )
    const pkgbuild = readFileSync(join(ROOT, 'packaging/arch/PKGBUILD'), 'utf8')
    const desktop = readFileSync(join(ROOT, `packaging/arch/${values.BRAND_DESKTOP_ID}.desktop`), 'utf8')
    expect(pkgbuild).toContain(`"\${pkgdir}/usr/bin/${values.BRAND_CLI}"`)
    expect(pkgbuild).toContain(`"\${pkgdir}/usr/share/applications/${values.BRAND_DESKTOP_ID}.desktop"`)
    expect(pkgbuild).toContain(`"\${pkgdir}/usr/bin/${values.BRAND_GUI_BINARY}"`)
    expect(desktop).toContain(`Name=${values.BRAND_PRODUCT}\n`)
    expect(desktop).toContain(`Exec=${values.BRAND_GUI_BINARY}\n`)
    expect(desktop).toContain(`Icon=${values.BRAND_DESKTOP_ID}\n`)
    expect(values.BRAND_CLI_ALIASES).toBe('k2so')
  })

  it('the app ships the CLI under the brand\'s command name and its aliases', () => {
    const conf = JSON.parse(readFileSync(join(ROOT, 'src-tauri/tauri.conf.json'), 'utf8'))
    for (const name of [CLI_NAME, ...CLI_ALIASES]) expect(conf.bundle.resources, name).toContain(`../cli/${name}`)
  })

  it('brand/k2\'s update feed is the release repo\'s, as k2-client builds it (APP_FEED_URL)', () => {
    const feed = JSON.parse(run(['tauri'])).plugins.updater.endpoints
    expect(feed).toEqual([`https://github.com/${RELEASE_REPO}/releases/latest/download/latest.json`])
    const rust = readFileSync(join(ROOT, 'crates/k2-client/src/brand.rs'), 'utf8')
    expect(rust).toContain('pub const APP_FEED_URL: &str = concat!("https://github.com/", release_repo!(), "/releases/latest/download/latest.json");')
  })

  it('a brand of its own gets its own overlay', () => {
    const dir = brandFolder('own', GOOD)
    expect(JSON.parse(run(['tauri', '--brand', dir]))).toEqual({
      productName: 'Acme Desk',
      identifier: 'com.example.acme',
      plugins: {
        updater: {
          endpoints: ['https://updates.example/acme/latest.json'],
          pubkey: Buffer.from(`untrusted comment: minisign public key: ${TEST_KEY.id}\n${TEST_KEY.line}\n`).toString('base64'),
        },
      },
    })
  })
})

describe('a brand file the build can\'t use stops it, naming the field', () => {
  it.each([
    ['no-cli', GOOD.replace(/\[cli\][\s\S]*?\]\n/, ''), '[cli] is missing'],
    ['bad-name', GOOD.replace('name = "ab"', 'name = "A B"'), 'cli.name "A B" must match'],
    ['bad-mode', GOOD.replace('"notice"', '"loud"'), 'cli.aliases[0].mode "loud" must be silent, notice or tombstone'],
    ['alias-self', GOOD.replace('{ name = "cd"', '{ name = "ab"'), 'cli.aliases[0].name "ab" is cli.name itself'],
    ['schema', GOOD.replace('schema = 1', 'schema = 2'), 'schema must be 1'],
    ['pkg-key', GOOD.replace('gui_binary', 'gui_bin'), '[package] holds a key it does not know: gui_bin'],
    ['pkg-feed', GOOD.replace('https://updates', 'http://updates'), 'package.update_feed must be an https URL'],
  ])('%s', (name, text, msg) => {
    const dir = brandFolder(name, text)
    expect(refused(['renderer', '--print', '--brand', dir])).toContain(msg)
  })

  it('a packaging build refuses a brand with no [package]; the renderer does not need one', () => {
    const dir = brandFolder('no-package', GOOD.replace(/\[package\][\s\S]*$/, ''))
    expect(run(['renderer', '--print', '--brand', dir])).toContain("export const CLI_NAME: string = 'ab'")
    expect(refused(['tauri', '--brand', dir])).toContain('[package] is missing')
    expect(refused(['shell', '--brand', dir])).toContain('[package] is missing')
  })

  it('a missing folder names K2_BRAND_DIR', () => {
    expect(refused(['renderer', '--print', '--brand', join(scratch, 'nowhere')])).toContain('K2_BRAND_DIR names the folder')
  })
})

describe('the update keys', () => {
  /** The key lines a bash copy assigns: NAME="line line". */
  function bashKeys(file: string, name: string): string[] {
    const m = readFileSync(join(ROOT, file), 'utf8').match(new RegExp(`^${name}="([^"]*)"$`, 'm'))
    if (!m) throw new Error(`no ${name}="..." in ${file}`)
    return m[1].split(' ')
  }
  /** brand/<id>'s [update] keys, as update-keys prints them. */
  function printed(args: string[]): { role: string; id: string; line: string }[] {
    return run(['update-keys', ...args])
      .trim()
      .split('\n')
      .filter((l) => !l.startsWith('brand-values:'))
      .map((l) => {
        const [role, id, line] = l.split(' ')
        return { role, id, line }
      })
  }

  it('every brand in the repo passes the ship check', () => {
    for (const id of readdirSync(join(ROOT, 'brand'), { withFileTypes: true }).filter((d) => d.isDirectory()).map((d) => d.name)) {
      expect(run(['update-keys', '--brand', `brand/${id}`]), id).toContain(`${id}'s update keys are its own`)
    }
  })

  it('brand/k2 carries exactly the ceremony\'s keys: k2-release-keys, tauri.conf.json and the two bash copies', () => {
    const keys = printed([])
    const rs = readFileSync(join(ROOT, 'crates/k2-release-keys/src/lib.rs'), 'utf8')
    const block = rs.slice(rs.indexOf('// BEGIN UPDATER_KEYS'), rs.indexOf('// END UPDATER_KEYS'))
    const inRust = [...block.matchAll(/role: "([a-z]+)",\s*key_id: "([0-9A-F]{16})",\s*public_key_b64: "([^"]+)"/g)].map((m) => ({ role: m[1], id: m[2], line: m[3] }))
    expect(inRust.length).toBeGreaterThan(0)
    expect(keys).toEqual(inRust)
    expect(keys[0]).toEqual({ role: 'primary', ...K2_PRIMARY })
    const conf = JSON.parse(readFileSync(join(ROOT, 'src-tauri/tauri.conf.json'), 'utf8'))
    expect(Buffer.from(conf.plugins.updater.pubkey, 'base64').toString()).toBe(`untrusted comment: minisign public key: ${keys[0].id}\n${keys[0].line}\n`)
    expect(bashKeys('cli/src/daemon-install.sh', '_DAEMON_UPDATE_PUBKEYS')).toEqual(keys.map((k) => k.line))
    expect(bashKeys('scripts/install-daemon.sh', 'K2_DAEMON_PUBKEYS')).toEqual(keys.map((k) => k.line))
  })

  it('brand/k5 carries the same keys as brand/k2', () => {
    expect(printed(['--brand', 'brand/k5'])).toEqual(printed([]))
  })

  it('brand/xy holds a placeholder key that is none of K2\'s', () => {
    const [k] = printed(['--brand', 'brand/xy'])
    expect(k.role).toBe('primary')
    for (const own of [...printed([]), { role: 'retired', ...K2_RETIRED }]) {
      expect(k.id).not.toBe(own.id)
      expect(k.line).not.toBe(own.line)
    }
  })

  it('brand/xy given K2\'s primary key on purpose is refused by the ship check, the overlay and the shell values', () => {
    const dir = xyWithKey('xy-our-key', K2_PRIMARY)
    const msg = `carries K2's update key ${K2_PRIMARY.id}; only k2 and k5 may ship it`
    expect(refused(['update-keys', '--brand', dir])).toContain(msg)
    expect(refused(['tauri', '--brand', dir])).toContain(msg)
    expect(refused(['shell', '--brand', dir])).toContain(msg)
  })

  it('brand/xy given the retired key U1 is refused too', () => {
    const dir = xyWithKey('xy-retired-key', K2_RETIRED)
    expect(refused(['update-keys', '--brand', dir])).toContain(`carries K2's update key ${K2_RETIRED.id}`)
  })

  it('brand/xy given its own fresh key passes', () => {
    const dir = xyWithKey('xy-fresh-key', throwawayKey())
    expect(run(['update-keys', '--brand', dir])).toContain("xy's update keys are its own")
  })

  it('a k2 or k5 brand with keys other than the ceremony\'s is refused', () => {
    const k2 = readFileSync(join(ROOT, 'brand/k2/brand.toml'), 'utf8')
    const dir = brandFolder('k2-other-key', k2.slice(0, k2.indexOf('[update]')) + updateTable(TEST_KEY))
    expect(refused(['update-keys', '--brand', dir])).toContain("brand k2 must carry exactly K2's update keys")
  })

  it.each([
    ['no-update', (t: string) => t.replace(/\[update\][\s\S]*?\n\]\n/, ''), '[update] is missing'],
    ['bad-line', (t: string) => t.replace(TEST_KEY.line, 'RWQnotakey'), 'update.keys[0].pubkey is not a minisign public key line'],
    ['bad-id', (t: string) => t.replace(`key_id = "${TEST_KEY.id}"`, 'key_id = "0000000000000000"'), `is not the pubkey's key id (${TEST_KEY.id})`],
    ['bad-role', (t: string) => t.replace('role = "primary"', 'role = "spare"'), 'must be primary or backup'],
  ])('%s is refused', (name, edit, msg) => {
    const dir = brandFolder(name, edit(GOOD))
    expect(refused(['update-keys', '--brand', dir])).toContain(msg)
  })
})
