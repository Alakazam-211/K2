import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { DASHBOARD_URL, ZONE, ZONE_SUFFIX } from './hosts'
import { SUBDOMAIN_HOST as PUBLISHED_HOST, PLAN_DASHBOARD_URL } from './published-subdomains'
import { SUBDOMAIN_HOST as URLS_HOST } from '@/components/WorkspacePanel/urls-ports'

const RUST = join(dirname(fileURLToPath(import.meta.url)), '../../../crates/k2-client/src/hosts.rs')

/** The value of `pub const NAME: &str = "...";` in the Rust hosts module. */
function rustConst(name: string): string {
  const src = readFileSync(RUST, 'utf8')
  const m = src.match(new RegExp(`^pub const ${name}: &str = "([^"]*)";$`, 'm'))
  if (!m) throw new Error(`no pub const ${name} in ${RUST}`)
  return m[1]
}

describe('the renderer hosts module', () => {
  it('holds the zone the Rust side holds', () => {
    expect(ZONE).toBe(rustConst('ZONE'))
    expect(ZONE_SUFFIX).toBe(rustConst('ZONE_SUFFIX'))
  })

  it('spells the values out, so a change here is seen', () => {
    expect(ZONE).toBe('k2.dev')
    expect(ZONE_SUFFIX).toBe('.k2.dev')
    expect(ZONE_SUFFIX).toBe(`.${ZONE}`)
    expect(DASHBOARD_URL).toBe('https://k2.dev/dashboard')
  })

  it('is where the older names take their values from', () => {
    expect(PUBLISHED_HOST).toBe(ZONE)
    expect(URLS_HOST).toBe(ZONE)
    expect(PLAN_DASHBOARD_URL).toBe(DASHBOARD_URL)
  })
})
