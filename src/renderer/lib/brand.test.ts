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
  RELEASE_REPO_NAMES,
  RELEASE_REPO_URL,
  RELEASES_URL_PREFIX,
  SUPPORT_EMAIL,
  hostedZones,
  isReleaseUrl,
  noteReportedBrand,
  releaseTagUrl,
  resetReportedBrandForTests,
} from './brand'

const HERE = dirname(fileURLToPath(import.meta.url))
const RUST = readFileSync(join(HERE, '../../../crates/k2-client/src/brand.rs'), 'utf8')
/** The default brand file: the command's name, its aliases and the product's name (k2-brand reads it). */
const BRAND_FILE = readFileSync(join(HERE, '../../../brand/k2/brand.toml'), 'utf8')
/** The bash homes of the release repo's name: the CLI's, the release tooling's and the signer's policy. */
const CLI_PRELUDE = readFileSync(join(HERE, '../../../cli/src/prelude.sh'), 'utf8')
const RELEASE_REPO_SH = readFileSync(join(HERE, '../../../scripts/lib/release-repo.sh'), 'utf8')
const SIGNER_POLICY = readFileSync(join(HERE, '../../../scripts/k2sign/policy'), 'utf8')

/** The value of a `NAME="..."` line in a bash file. */
function bashVar(text: string, name: string, file: string): string {
  const m = text.match(new RegExp(`^${name}="([^"]*)"$`, 'm'))
  if (!m) throw new Error(`no ${name}="..." in ${file}`)
  return m[1]
}

/** The value of a `key = value` line in the signer's policy. */
function policyValue(key: string): string {
  const m = SIGNER_POLICY.match(new RegExp(`^${key} = (.*)$`, 'm'))
  if (!m) throw new Error(`no ${key} in scripts/k2sign/policy`)
  return m[1].trim()
}

/** The strings in brand.rs's `pub const RELEASE_REPO_NAMES: &[&str] = &[...];`. */
function rustRepoNames(): string[] {
  const m = RUST.match(/^pub const RELEASE_REPO_NAMES: &\[&str\] = &\[(.*)\];$/m)
  if (!m) throw new Error('no one-line RELEASE_REPO_NAMES in brand.rs')
  return [...m[1].matchAll(/"([^"]*)"/g)].map((x) => x[1])
}

/** The body of one `[table]` of the default brand file. */
function brandTable(name: string): string {
  const m = BRAND_FILE.match(new RegExp(`^\\[${name}\\]\\n([\\s\\S]*?)(?=^\\[[a-z]+\\]$|(?![\\s\\S]))`, 'm'))
  if (!m) throw new Error(`no [${name}] in brand/k2/brand.toml`)
  return m[1]
}

/** The value of `key = "..."` in one table of the default brand file. */
function brandString(table: string, key: string): string {
  const m = brandTable(table).match(new RegExp(`^${key} = "([^"]*)"$`, 'm'))
  if (!m) throw new Error(`no ${table}.${key} in brand/k2/brand.toml`)
  return m[1]
}

/** The names in `[cli] aliases = [{ name = "...", mode = "..." }, ...]`, in order. */
function brandAliases(): string[] {
  const m = brandTable('cli').match(/^aliases = \[(.*)\]$/m)
  if (!m) throw new Error('no one-line cli.aliases in brand/k2/brand.toml')
  const names = [...m[1].matchAll(/\{ name = "([^"]*)", mode = "[a-z]+" \}/g)].map((x) => x[1])
  const entries = m[1].split('{').length - 1
  if (names.length !== entries) throw new Error(`cli.aliases has ${entries} entries, read ${names.length}`)
  return names
}

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
    expect(PRODUCT_NAME).toBe(brandString('names', 'product'))
    expect(CLI_NAME).toBe(brandString('cli', 'name'))
    expect(CLI_ALIASES).toEqual(brandAliases())
    expect(RELEASE_REPO).toBe(rustMacro('release_repo'))
    expect(RELEASE_REPO_NAMES).toEqual(rustRepoNames())
    expect(COMPANY_NAME).toBe(rustConst('COMPANY_NAME'))
    expect(PUBLIC_SITE).toBe(rustConst('PUBLIC_SITE'))
    expect(SUPPORT_EMAIL).toBe(rustConst('SUPPORT_EMAIL'))
  })

  it('one repo name everywhere: the CLI, the release tooling and the signer agree with brand.rs', () => {
    const repo = rustMacro('release_repo')
    expect(bashVar(CLI_PRELUDE, '_K2_RELEASE_REPO', 'cli/src/prelude.sh')).toBe(repo)
    expect(bashVar(RELEASE_REPO_SH, 'K2_RELEASE_REPO_DEFAULT', 'scripts/lib/release-repo.sh')).toBe(repo)
    expect(bashVar(RELEASE_REPO_SH, 'K2_RELEASE_REPO_NAMES', 'scripts/lib/release-repo.sh').split(' ')).toEqual(rustRepoNames())
    expect(policyValue('repo')).toBe(repo)
    expect(policyValue('repo_names').split(/\s+/)).toEqual(rustRepoNames())
  })

  it('a release URL under either of the repo\'s names counts, and no other', () => {
    for (const name of RELEASE_REPO_NAMES) {
      expect(isReleaseUrl(`https://github.com/${name}/releases/tag/v9.9.9`), name).toBe(true)
      expect(isReleaseUrl(`https://github.com/${name}/releases/download/v9.9.9/K2.app.tar.gz`), name).toBe(true)
    }
    for (const url of [
      'https://github.com/AlakazamLabs/K2/releases/tag/v9.9.9',
      'https://github.com/Alakazam-211/K5/releases/tag/v9.9.9',
      'https://github.com/Alakazam-211/K2SO/releases/tag/v9.9.9',
      'https://github.com/AlakazamLabs/K5-fork/releases/tag/v9.9.9',
      'http://github.com/Alakazam-211/K2/releases/tag/v9.9.9',
      'https://github.com/Alakazam-211/K2/blob/main/README.md',
      'https://evil.example/https://github.com/Alakazam-211/K2/releases/',
    ]) {
      expect(isReleaseUrl(url), url).toBe(false)
    }
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
    expect(RELEASE_REPO_NAMES).toEqual(['Alakazam-211/K2', 'AlakazamLabs/K5'])
    expect(RELEASE_REPO_NAMES).toContain(RELEASE_REPO)
    expect(RELEASE_REPO_URL).toBe(`https://github.com/${RELEASE_REPO}`)
    expect(RELEASES_URL_PREFIX).toBe(`https://github.com/${RELEASE_REPO}/releases/`)
    expect(releaseTagUrl('0.46.14')).toBe(`https://github.com/${RELEASE_REPO}/releases/tag/v0.46.14`)
    expect(releaseTagUrl('v0.46.14')).toBe(`https://github.com/${RELEASE_REPO}/releases/tag/v0.46.14`)
  })

  it('answers with the zones constant until a server says otherwise', () => {
    expect(hostedZones()).toEqual(HOSTED_ZONES)
  })

  it('keeps the zones a full /boot-status body reports', () => {
    noteReportedBrand({ hostedZones: ['a.example', 'b.example'] })
    expect(hostedZones()).toEqual(['a.example', 'b.example'])
  })

  it('an older body, or a field of the wrong shape, changes nothing', () => {
    noteReportedBrand({ hostedZones: ['a.example'] })
    noteReportedBrand({})
    noteReportedBrand(null)
    noteReportedBrand({ hostedZones: [] })
    noteReportedBrand({ hostedZones: ['ok.example', 3] })
    expect(hostedZones()).toEqual(['a.example'])
  })

  it('the names are the build\'s: nothing asks a server for them', async () => {
    const exported = Object.keys(await import('./brand'))
    expect(exported).not.toContain('productName')
    expect(exported).not.toContain('cliName')
    expect(exported).toEqual(expect.arrayContaining(['BRAND_ID', 'PRODUCT_NAME', 'CLI_NAME', 'CLI_ALIASES']))
  })
})
