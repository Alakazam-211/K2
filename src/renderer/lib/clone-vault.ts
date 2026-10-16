// Vault secrets that ride a "Clone to" / "Clone to this computer" move
// (0.45.2). The daemons do the work: the destination names its vault
// (`GET vault/recipient`), the source seals the workspace's vault secrets
// to it inside `clone/bundle` / `clone/pack`, and the destination's
// `clone/unpack` adds them to the newly registered workspace. This module
// only carries those opaque values between the two daemons and turns the
// answers into a names-only note for the done screen. Values never pass
// through here.
//
// Only for the owner moving their OWN workspace to their OWN server: the
// flows send `own_box_move: true` only when the dialog's checkbox is ticked,
// and the source daemon refuses a vault target without it.

/** The destination vault, as `vault/recipient` answers it (wire shape). */
export interface VaultRecipientResponse {
  recipient?: string
  fingerprint?: string
  vaultId?: string
}

/** What the source's `clone/bundle` / `clone/pack` takes. */
export interface CloneVaultTarget {
  recipient: string
  vault_id: string
}

/** The source's names-only report (`vault_carry`). */
export interface CloneVaultCarryReport {
  sealed: boolean
  count: number
  secrets: string[]
  reason?: string
  note?: string
}

/** The destination's answer fields on `clone/unpack`. */
export interface CloneVaultUnpackFields {
  vault_imported?: string[]
  vault_skipped?: string[]
  vault_error?: { code?: string; hint?: string }
}

/** What the done screen shows: names only, plus a note when nothing (or
 *  not everything) moved. */
export interface CloneVaultOutcome {
  imported: string[]
  skipped: string[]
  note: string | null
}

export type VaultRecipientLookup = { target: CloneVaultTarget } | { note: string }

/** The daemon's vault refusals arrive as `{"error":{"code","hint"}}`, which
 *  the request layer surfaces as the raw JSON text. */
function vaultRefusal(message: string): { code?: string; hint?: string } | null {
  try {
    const parsed = JSON.parse(message) as { error?: { code?: string; hint?: string } }
    if (parsed && typeof parsed.error === 'object' && parsed.error) return parsed.error
  } catch {
    /* not a vault refusal */
  }
  return null
}

/**
 * Ask the destination for its vault recipient. Never throws: an older
 * destination (no such route), a refusal (not an Owner/Admin there, vault
 * closed) or an unreachable server all become a note, and the clone goes
 * on without vault secrets.
 */
export async function askVaultRecipient(
  get: () => Promise<VaultRecipientResponse>,
  destLabel: string,
): Promise<VaultRecipientLookup> {
  try {
    const r = await get()
    if (r && typeof r.recipient === 'string' && r.recipient && typeof r.vaultId === 'string' && r.vaultId) {
      return { target: { recipient: r.recipient, vault_id: r.vaultId } }
    }
    return { note: `${destLabel} didn't name its vault, so vault secrets weren't moved.` }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    const refusal = vaultRefusal(message)
    if (refusal) {
      return {
        note: `Vault secrets weren't moved: ${destLabel} said ${refusal.hint ?? refusal.code ?? 'no'}.`,
      }
    }
    if (/not found/i.test(message)) {
      return {
        note: `${destLabel} runs an older K2 that can't receive vault secrets yet. Update it, then add this workspace's secrets there.`,
      }
    }
    return { note: `Couldn't ask ${destLabel} for its vault (${message}), so vault secrets weren't moved.` }
  }
}

/** The fields to add to the source's `clone/bundle` / `clone/pack` body. */
export function vaultBundleFields(lookup: VaultRecipientLookup | null): Record<string, unknown> {
  if (!lookup || !('target' in lookup)) return {}
  return { vault_target: lookup.target, own_box_move: true }
}

/** The field to add to the destination's `clone/unpack` body: the sealed
 *  carry, only when this run asked for one. */
export function vaultUnpackFields(
  lookup: VaultRecipientLookup | null,
  carryB64: string | undefined,
): Record<string, unknown> {
  if (!lookup || !('target' in lookup) || !carryB64) return {}
  return { vault_carry_b64: carryB64 }
}

/** Fold the three answers into the done screen's outcome. `null` when the
 *  checkbox was off (nothing to show). */
export function vaultOutcome(
  lookup: VaultRecipientLookup | null,
  report: CloneVaultCarryReport | undefined,
  carryB64: string | undefined,
  unpack: CloneVaultUnpackFields | undefined,
): CloneVaultOutcome | null {
  if (!lookup) return null
  if ('note' in lookup) return { imported: [], skipped: [], note: lookup.note }
  if (!carryB64) {
    return {
      imported: [],
      skipped: [],
      note: report?.note ?? 'The source server didn’t send any vault secrets (it may run an older K2).',
    }
  }
  if (unpack?.vault_error) {
    return {
      imported: [],
      skipped: [],
      note: `Vault secrets weren't added: ${unpack.vault_error.hint ?? unpack.vault_error.code ?? 'unknown error'}`,
    }
  }
  if (!Array.isArray(unpack?.vault_imported)) {
    return { imported: [], skipped: [], note: 'The destination didn’t report adding the vault secrets.' }
  }
  return {
    imported: unpack.vault_imported,
    skipped: Array.isArray(unpack.vault_skipped) ? unpack.vault_skipped : [],
    note: null,
  }
}
