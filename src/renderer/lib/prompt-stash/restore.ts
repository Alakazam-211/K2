// stash-sync-v1 (0.46.7) — putting a stash's files back in a composer.
//
// Rosson (10-30): a stash only tracks which file was going to be sent and
// from which device (its path there), never the file itself. So a file
// comes back only on the device that saved it, and only if it is still
// there; otherwise the composer says why, per file:
//   "shot.png wasn't restored: it's on Rosson's MacBook"
//   "shot.png wasn't restored: it's no longer at /Users/…/shot.png"
//   "shot.png wasn't restored: it's gone from this device"
// Device names come from the Homes device registry; an id it doesn't list
// (unknown or removed) is "another device". Not in v1: fetching a file
// through the server.

import type { StashAttachment, StashRecord } from './record'

export interface RestoreDeps {
  /** This device's id (null: unknown, so only pre-sync files are ours). */
  deviceId: string | null
  /** The registry's names by device id (fetched only when needed). */
  deviceNames: () => Promise<ReadonlyMap<string, string>>
  /** Does `path` exist on this computer? null = couldn't check. */
  pathExists: (path: string) => Promise<boolean | null>
  /** The bytes this device kept for `att`, or null when gone. */
  readBytes: (att: StashAttachment) => Promise<File | null>
}

export interface RestorePlan {
  /** Files to attach from this device's own copy. */
  files: File[]
  /** Paths on this computer to attach (the kept copy was gone). */
  paths: string[]
  /** One line per file that stayed behind. */
  notes: string[]
}

/** The name the composer shows for another device. */
export function deviceLabel(names: ReadonlyMap<string, string>, id: string): string {
  return names.get(id)?.trim() || 'another device'
}

export async function planStashRestore(rec: Pick<StashRecord, 'attachments'>, deps: RestoreDeps): Promise<RestorePlan> {
  const plan: RestorePlan = { files: [], paths: [], notes: [] }
  let names: ReadonlyMap<string, string> | null = null
  for (const att of rec.attachments) {
    const owner = att.device_id ?? null
    // A file saved on another device stays there.
    if (owner !== null && owner !== deps.deviceId) {
      if (!names) {
        try {
          names = await deps.deviceNames()
        } catch (err) {
          console.warn('[stash] device names:', err)
          names = new Map()
        }
      }
      plan.notes.push(`${att.name} wasn't restored: it's on ${deviceLabel(names, owner)}`)
      continue
    }
    // This device's (or a pre-sync record's) file with a path: it must
    // still be at that path.
    const path = att.path ?? null
    if (path) {
      const exists = await deps.pathExists(path)
      if (exists === false) {
        plan.notes.push(`${att.name} wasn't restored: it's no longer at ${path}`)
        continue
      }
      const file = await deps.readBytes(att)
      if (file) plan.files.push(file)
      else if (exists) plan.paths.push(path)
      else plan.notes.push(`${att.name} wasn't restored: it's gone from this device`)
      continue
    }
    const file = await deps.readBytes(att)
    if (file) plan.files.push(file)
    else plan.notes.push(`${att.name} wasn't restored: it's gone from this device`)
  }
  return plan
}
