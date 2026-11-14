/**
 * The zone K2 boxes are named under, in one place for the renderer.
 *
 * The same values as `crates/k2-client/src/hosts.rs` (`k2_core::hosts`);
 * `hosts.test.ts` reads that file and fails when the two drift. The
 * daemon decides what a host means; the renderer only needs the zone to
 * show a name before the daemon has answered, and to tidy what a person
 * typed.
 */

/** The zone itself. */
export const ZONE = 'k2.dev'

/** The zone as a suffix, with its dot: what a box's full name ends with. */
export const ZONE_SUFFIX = '.k2.dev'

/** Where a person manages their plan. */
export const DASHBOARD_URL = 'https://k2.dev/dashboard'

/** The default relay a tunnel dials (the daemon's `hosts::RELAY_HOST`). */
export const RELAY_HOST = 'relay.k2.dev'

/** `<label>.k2.dev`. The label is used as given, as a template string
 *  would use it. */
export function boxHost(label: string | null | undefined): string {
  return `${label}${ZONE_SUFFIX}`
}
