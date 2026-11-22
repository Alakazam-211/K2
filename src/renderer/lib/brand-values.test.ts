// scripts/brand-values.mjs: the brand's values for the renderer and the
// packagers, read from the brand file when building. These tests run the
// script the way a build does and read what it prints.

import { execFileSync, spawnSync } from 'node:child_process'
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
].join('\n')

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
      plugins: { updater: { endpoints: conf.plugins.updater.endpoints } },
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
      plugins: { updater: { endpoints: ['https://updates.example/acme/latest.json'] } },
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
