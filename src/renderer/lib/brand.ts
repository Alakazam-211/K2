/**
 * The product's names, one definition each, for the renderer.
 *
 * The product's name, the command's name and its aliases, the company, the
 * lockup, the public site, the support address and the contact address come from the brand file (`brand/<id>/brand.toml`, `brand/k2` unless `K2_BRAND_DIR`
 * names another) through `brand.generated.ts`, which
 * `scripts/brand-values.mjs` writes when building; Rust reads the same file
 * through crates/k2-brand. Two names, never made from each other: the
 * product's for prose and screens, the command's for anything typed or
 * copied. Nothing here asks a server for either: a build shows the names
 * it was built with.
 *
 * `brand.test.ts` checks them against the brand file; the rest are the same
 * values as `crates/k2-client/src/brand.rs` (`k2_core::brand`), and it reads
 * both and fails when they drift.
 * Copy that names the product, the command, the site or the release repo
 * uses these instead of spelling the name out; `scripts/brand-gate.sh`
 * keeps new literals out of the rest of the renderer.
 *
 * A daemon reports its `hostedZones` in the full `/boot-status` body.
 * `noteReportedBrand` keeps what the window's own server said, and
 * `hostedZones()` answers with that, or with the constant here when the
 * daemon is older and did not say.
 */
import { ZONE } from './hosts'
import { PRODUCT_NAME } from './brand.generated'

export {
  BRAND_ID,
  CLI_ALIASES,
  CLI_NAME,
  COMPANY_NAME,
  CONTACT_EMAIL,
  PRODUCT_LOCKUP,
  PRODUCT_NAME,
  PUBLIC_SITE,
  SUPPORT_EMAIL,
} from './brand.generated'

/** The product's name as a RegExp source (special characters escaped), for
 *  reading a server's text that names the product. */
export const PRODUCT_NAME_PATTERN = PRODUCT_NAME.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/** The zone boxes are named under (the hosts module's zone). */
export const INFRA_ZONE = ZONE

/** The zones boxes are served under, primary first, when the daemon has
 *  not said (`/boot-status` `hostedZones`). */
export const HOSTED_ZONES: readonly string[] = [ZONE]

/** The repo releases come from, `owner/name`: every release URL is built
 *  from this. The repo is moving from Alakazam-211/K2 to AlakazamLabs/K5 (a
 *  transfer and rename, which GitHub redirects); this stays the old name
 *  until the move, and on move day this line is the whole change here. */
export const RELEASE_REPO = 'Alakazam-211/K2'

/** Every name the release repo answers to, `owner/name`: its name before the
 *  move and its name after. A check that compares a name accepts exactly
 *  these, never any other; a URL is built from {@link RELEASE_REPO}. */
export const RELEASE_REPO_NAMES: readonly string[] = ['Alakazam-211/K2', 'AlakazamLabs/K5']

/** The repo's page. */
export const RELEASE_REPO_URL = `https://github.com/${RELEASE_REPO}`

/** The prefix every release page and release file URL starts with. */
export const RELEASES_URL_PREFIX = `${RELEASE_REPO_URL}/releases/`

/** `url` is a release page or a release file of the release repo, under
 *  either of its names. */
export function isReleaseUrl(url: string): boolean {
  return RELEASE_REPO_NAMES.some((name) => url.startsWith(`https://github.com/${name}/releases/`))
}

/** The page of one release (`v` is added; a leading one is not doubled). */
export function releaseTagUrl(version: string): string {
  return `${RELEASES_URL_PREFIX}tag/v${version.startsWith('v') ? version.slice(1) : version}`
}

interface ReportedBrand {
  hostedZones?: readonly string[]
}

let reported: ReportedBrand = {}

/** Keep the hosted zones a full `/boot-status` body carries. A body without
 *  them (an older daemon, or a stranger's partial view) changes nothing, and
 *  a field of the wrong shape is ignored. The body's `productName` and
 *  `cliName` are not read: the renderer shows the names it was built with. */
export function noteReportedBrand(body: { hostedZones?: unknown } | null | undefined): void {
  if (!body) return
  const next: ReportedBrand = { ...reported }
  const zones = body.hostedZones
  if (Array.isArray(zones) && zones.length > 0 && zones.every((z) => typeof z === 'string' && z)) {
    next.hostedZones = [...zones] as string[]
  }
  reported = next
}

/** The zones the server reported, else {@link HOSTED_ZONES}. */
export function hostedZones(): readonly string[] {
  return reported.hostedZones ?? HOSTED_ZONES
}

/** For tests: forget what a server reported. */
export function resetReportedBrandForTests(): void {
  reported = {}
}
