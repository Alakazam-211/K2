// Settings → Compute → CI on K2 Compute (preview), 0.46.17: the server
// switch `ciPreview`. The daemon owns it: read from `/cli/settings/get`,
// written through the POST-only `/cli/settings/update` (an owner-only key:
// Admins, Members and agents are refused there). The switch takes effect at
// once; CI also needs compute on (`computePreview`).

import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'
import { daemonCliGet } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { roleFromWhoami, type HostRole } from './host-role'

/** The settings key, exactly as the daemon stores it. */
export const CI_PREVIEW_KEY = 'ciPreview'

/** Whether the window's server has CI on K2 Compute on (absent = off). */
export async function fetchCiPreview(): Promise<boolean> {
  const s = await settingsGet()
  return s.ciPreview === true
}

/** Turn it on or off; resolves with what the server now stores. Throws the
 *  server's refusal (a non-owner gets 403). */
export async function saveCiPreview(on: boolean): Promise<boolean> {
  const s = await settingsUpdate({ [CI_PREVIEW_KEY]: on })
  if (typeof s.ciPreview !== 'boolean') {
    throw new Error('the server answered without ciPreview')
  }
  return s.ciPreview
}

/** The viewer's role on the window's server (`auth/whoami`), or null when
 *  it couldn't be read: the switch then stays read-only. */
export async function fetchCiPreviewRole(): Promise<HostRole | null> {
  try {
    return roleFromWhoami(await daemonCliGet<unknown>(primaryScope(), 'auth/whoami'))
  } catch {
    return null
  }
}

/** A refused or failed write, in plain words. */
export function ciPreviewErrorText(err: unknown): string {
  const msg = err instanceof Error ? err.message : String(err)
  if (/\b403\b/.test(msg)) return 'only this server’s owner can change this'
  return msg
}
