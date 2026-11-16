/**
 * The product's names, one definition each, for the renderer.
 *
 * The same values as `crates/k2-client/src/brand.rs` (`k2_core::brand`);
 * `brand.test.ts` reads that file and fails when the two drift. Copy that
 * names the product, the command, the site or the release repo uses these
 * instead of spelling the name out; `scripts/brand-gate.sh` keeps new
 * literals out of the rest of the renderer.
 *
 * A daemon reports its own `productName`, `cliName` and `hostedZones` in
 * the full `/boot-status` body. `noteReportedBrand` keeps what the window's
 * own server said, and `productName()` / `cliName()` / `hostedZones()`
 * answer with that, or with the constants here when the daemon is older
 * and did not say.
 */
import { ZONE } from './hosts'

/** The product's name, as people read it. */
export const PRODUCT_NAME = 'K2'

/** The company that makes it. */
export const COMPANY_NAME = 'Alakazam Labs'

/** The product with its maker. */
export const PRODUCT_LOCKUP = `${PRODUCT_NAME} by ${COMPANY_NAME}`

/** The command people and agents type. */
export const CLI_NAME = 'k2'

/** Other names the command answers to (the pre-0.40 shim). */
export const CLI_ALIASES: readonly string[] = ['k2so']

/** The public website. */
export const PUBLIC_SITE = 'https://k2.dev'

/** The zone boxes are named under (the hosts module's zone). */
export const INFRA_ZONE = ZONE

/** The zones boxes are served under, primary first, when the daemon has
 *  not said (`/boot-status` `hostedZones`). */
export const HOSTED_ZONES: readonly string[] = [ZONE]

/** Where people write for help. */
export const SUPPORT_EMAIL = 'support@k2.dev'

/** The repo releases come from, `owner/name`. */
export const RELEASE_REPO = 'Alakazam-211/K2'

/** The repo's page. */
export const RELEASE_REPO_URL = `https://github.com/${RELEASE_REPO}`

/** The prefix every release page and release file URL starts with. */
export const RELEASES_URL_PREFIX = `${RELEASE_REPO_URL}/releases/`

/** The page of one release (`v` is added; a leading one is not doubled). */
export function releaseTagUrl(version: string): string {
  return `${RELEASES_URL_PREFIX}tag/v${version.startsWith('v') ? version.slice(1) : version}`
}

interface ReportedBrand {
  productName?: string
  cliName?: string
  hostedZones?: readonly string[]
}

let reported: ReportedBrand = {}

/** Keep the names a full `/boot-status` body carries. A body without them
 *  (an older daemon, or a stranger's partial view) changes nothing, and a
 *  field of the wrong shape is ignored. */
export function noteReportedBrand(body: { productName?: unknown; cliName?: unknown; hostedZones?: unknown } | null | undefined): void {
  if (!body) return
  const next: ReportedBrand = { ...reported }
  if (typeof body.productName === 'string' && body.productName) next.productName = body.productName
  if (typeof body.cliName === 'string' && body.cliName) next.cliName = body.cliName
  const zones = body.hostedZones
  if (Array.isArray(zones) && zones.length > 0 && zones.every((z) => typeof z === 'string' && z)) {
    next.hostedZones = [...zones] as string[]
  }
  reported = next
}

/** The product's name as the server reported it, else {@link PRODUCT_NAME}. */
export function productName(): string {
  return reported.productName ?? PRODUCT_NAME
}

/** The command's name as the server reported it, else {@link CLI_NAME}. */
export function cliName(): string {
  return reported.cliName ?? CLI_NAME
}

/** The zones the server reported, else {@link HOSTED_ZONES}. */
export function hostedZones(): readonly string[] {
  return reported.hostedZones ?? HOSTED_ZONES
}

/** For tests: forget what a server reported. */
export function resetReportedBrandForTests(): void {
  reported = {}
}
