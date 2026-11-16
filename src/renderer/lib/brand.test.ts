import { afterEach, describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  CLI_ALIASES,
  CLI_NAME,
  COMPANY_NAME,
  HOSTED_ZONES,
  INFRA_ZONE,
  PRODUCT_LOCKUP,
  PRODUCT_NAME,
  PUBLIC_SITE,
  RELEASE_REPO,
  RELEASE_REPO_URL,
  RELEASES_URL_PREFIX,
  SUPPORT_EMAIL,
  cliName,
  hostedZones,
  noteReportedBrand,
  productName,
  releaseTagUrl,
  resetReportedBrandForTests,
} from './brand'

const RUST = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '../../../crates/k2-client/src/brand.rs'), 'utf8')

/** The literal a Rust brand macro (`product_name!()`) expands to. */
function rustMacro(name: string): string {
  const m = RUST.match(new RegExp(`macro_rules! ${name} \\{\\s*\\(\\) => \\{\\s*"([^"]*)"`))
  if (!m) throw new Error(`no macro ${name} in brand.rs`)
  return m[1]
}

/** The value of `pub const NAME: &str = "...";` in brand.rs. */
function rustConst(name: string): string {
  const m = RUST.match(new RegExp(`^pub const ${name}: &str = "([^"]*)";$`, 'm'))
  if (!m) throw new Error(`no pub const ${name} in brand.rs`)
  return m[1]
}

afterEach(() => resetReportedBrandForTests())

describe('the renderer brand module', () => {
  it('holds the names the Rust side holds', () => {
    expect(PRODUCT_NAME).toBe(rustMacro('product_name'))
    expect(CLI_NAME).toBe(rustMacro('cli_name'))
    expect(RELEASE_REPO).toBe(rustMacro('release_repo'))
    expect(COMPANY_NAME).toBe(rustConst('COMPANY_NAME'))
    expect(PUBLIC_SITE).toBe(rustConst('PUBLIC_SITE'))
    expect(SUPPORT_EMAIL).toBe(rustConst('SUPPORT_EMAIL'))
    expect(RUST).toContain(`pub const CLI_ALIASES: &[&str] = &[${CLI_ALIASES.map((a) => `"${a}"`).join(', ')}];`)
  })

  it('spells the values out, so a change here is seen', () => {
    expect(PRODUCT_NAME).toBe('K2')
    expect(COMPANY_NAME).toBe('Alakazam Labs')
    expect(PRODUCT_LOCKUP).toBe('K2 by Alakazam Labs')
    expect(CLI_NAME).toBe('k2')
    expect(CLI_ALIASES).toEqual(['k2so'])
    expect(PUBLIC_SITE).toBe('https://k2.dev')
    expect(INFRA_ZONE).toBe('k2.dev')
    expect(HOSTED_ZONES).toEqual(['k2.dev'])
    expect(SUPPORT_EMAIL).toBe('support@k2.dev')
    expect(RELEASE_REPO).toBe('Alakazam-211/K2')
    expect(RELEASE_REPO_URL).toBe('https://github.com/Alakazam-211/K2')
    expect(RELEASES_URL_PREFIX).toBe('https://github.com/Alakazam-211/K2/releases/')
    expect(releaseTagUrl('0.46.14')).toBe('https://github.com/Alakazam-211/K2/releases/tag/v0.46.14')
    expect(releaseTagUrl('v0.46.14')).toBe('https://github.com/Alakazam-211/K2/releases/tag/v0.46.14')
  })

  it('answers with the constants until a server says otherwise', () => {
    expect(productName()).toBe(PRODUCT_NAME)
    expect(cliName()).toBe(CLI_NAME)
    expect(hostedZones()).toEqual(HOSTED_ZONES)
  })

  it('keeps what a full /boot-status body reports', () => {
    noteReportedBrand({ productName: 'P', cliName: 'p', hostedZones: ['a.example', 'b.example'] })
    expect(productName()).toBe('P')
    expect(cliName()).toBe('p')
    expect(hostedZones()).toEqual(['a.example', 'b.example'])
  })

  it('an older body, or a field of the wrong shape, changes nothing', () => {
    noteReportedBrand({ productName: 'P', cliName: 'p', hostedZones: ['a.example'] })
    noteReportedBrand({})
    noteReportedBrand(null)
    noteReportedBrand({ productName: '', cliName: 7, hostedZones: [] })
    noteReportedBrand({ hostedZones: ['ok.example', 3] })
    expect(productName()).toBe('P')
    expect(cliName()).toBe('p')
    expect(hostedZones()).toEqual(['a.example'])
  })
})
