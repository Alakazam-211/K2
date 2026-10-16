// Vault secrets that ride a clone (0.46.0, prd-clone-carries-threads-and-
// vault-v1 §6). The daemons do the work: the destination names its vault
// (`GET vault/recipient`), the source builds the per-secret plan
// (`POST clone/vault/plan`), takes the person's `clone-carry` confirm for
// any copy, and seals the copied secrets (plus names-only blank rows) to
// the destination inside `clone/bundle` / `clone/pack`; the destination's
// `clone/unpack` adds them to the newly registered workspace by id and
// posts the leave-blank cards. This module only carries those opaque
// values between the two daemons and turns the answers into a names-only
// note for the done screen. Values never pass through here.

/** The destination vault, as `vault/recipient` answers it (wire shape). */
export interface VaultRecipientResponse {
  recipient?: string
  fingerprint?: string
  vaultId?: string
}

/** What the source's `clone/bundle` / `clone/pack` takes as `vault_target`.
 *  `label` (what people call the destination) is shown in the Touch ID
 *  reason and is part of the confirmed plan. */
export interface CloneVaultTarget {
  recipient: string
  vault_id: string
  label?: string
}

/** One secret's choice in the Secrets block. */
export type CloneSecretChoice = 'copy' | 'blank' | 'skip'

/** The rest of the vault half of a bundle/pack request (from the dialog). */
export interface CloneVaultOptions {
  /** The clone id the confirm was made for (also the manifest's). */
  cloneId?: string
  /** Per-secret choices by name; unnamed rows keep their defaults. */
  secrets?: Record<string, CloneSecretChoice>
  /** The `clone-carry` confirm token (any copy needs one). */
  confirmToken?: string
  /** "This is my own server/computer": keeps person-given marks. */
  ownBoxMove?: boolean
  /** The destination vault the dialog already asked for (the confirm was
   *  minted for it): skips the second recipient lookup. */
  target?: CloneVaultTarget
}

/** The source's names-only report (`vault_carry`). */
export interface CloneVaultCarryReport {
  sealed: boolean
  copied?: string[]
  blank?: string[]
  skipped?: string[]
  notCarried?: { name: string; kind: string; reason: string; note: string }[]
  cloneId?: string
  reason?: string
  error?: { code?: string; hint?: string }
  /** Older (0.45.x held) shape. */
  note?: string
}

/** The destination's answer fields on `clone/unpack`. */
export interface CloneVaultUnpackFields {
  vault_imported?: string[]
  vault_skipped?: string[]
  vault_grants?: number
  vault_cards_posted?: string[]
  vault_cards_deferred?: string[]
  vault_cards_overflow?: string[]
  vault_error?: { code?: string; hint?: string }
}

/** What the done screen shows: names only, plus a note when nothing (or
 *  not everything) moved. */
export interface CloneVaultOutcome {
  imported: string[]
  skipped: string[]
  /** Left blank: a secret card asks for each on the clone's Thread. */
  blank: string[]
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
 * destination (no such route), a refusal (vault closed) or an unreachable
 * server all become a note, and the clone goes on without vault secrets.
 */
export async function askVaultRecipient(
  get: () => Promise<VaultRecipientResponse>,
  destLabel: string,
): Promise<VaultRecipientLookup> {
  try {
    const r = await get()
    if (r && typeof r.recipient === 'string' && r.recipient && typeof r.vaultId === 'string' && r.vaultId) {
      return { target: { recipient: r.recipient, vault_id: r.vaultId, label: destLabel } }
    }
    return { note: `${destLabel} didn't name its vault, so vault secrets weren't copied.` }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    const refusal = vaultRefusal(message)
    if (refusal) {
      return {
        note: `Vault secrets weren't copied: ${destLabel} said ${refusal.hint ?? refusal.code ?? 'no'}.`,
      }
    }
    if (/not found/i.test(message)) {
      return {
        note: `${destLabel} runs an older K2 that can't receive vault secrets yet. Update it, then add this workspace's secrets there.`,
      }
    }
    return { note: `Couldn't ask ${destLabel} for its vault (${message}), so vault secrets weren't copied.` }
  }
}

/** The fields to add to the source's `clone/bundle` / `clone/pack` body. */
export function vaultBundleFields(
  lookup: VaultRecipientLookup | null,
  opts: CloneVaultOptions = {},
): Record<string, unknown> {
  if (!lookup || !('target' in lookup)) return {}
  const out: Record<string, unknown> = { vault_target: lookup.target }
  if (opts.cloneId) out.clone_id = opts.cloneId
  if (opts.secrets && Object.keys(opts.secrets).length > 0) out.secrets = opts.secrets
  if (opts.confirmToken) out.confirm_token = opts.confirmToken
  if (opts.ownBoxMove) out.own_box_move = true
  return out
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
 *  vault wasn't asked (nothing to show). */
export function vaultOutcome(
  lookup: VaultRecipientLookup | null,
  report: CloneVaultCarryReport | undefined,
  carryB64: string | undefined,
  unpack: CloneVaultUnpackFields | undefined,
): CloneVaultOutcome | null {
  if (!lookup) return null
  const blank = Array.isArray(report?.blank) ? report.blank : []
  if ('note' in lookup) return { imported: [], skipped: [], blank: [], note: lookup.note }
  if (!carryB64) {
    const why = report?.error?.hint ?? report?.note
    return {
      imported: [],
      skipped: [],
      blank: [],
      note:
        why ??
        (report?.reason === 'no_secrets'
          ? 'This workspace has no vault secrets to copy.'
          : 'The source server didn’t send any vault secrets (it may run an older K2).'),
    }
  }
  if (unpack?.vault_error) {
    return {
      imported: [],
      skipped: [],
      blank: [],
      note: `Vault secrets weren't added: ${unpack.vault_error.hint ?? unpack.vault_error.code ?? 'unknown error'}`,
    }
  }
  if (!Array.isArray(unpack?.vault_imported)) {
    return { imported: [], skipped: [], blank: [], note: 'The destination didn’t report adding the vault secrets.' }
  }
  return {
    imported: unpack.vault_imported,
    skipped: Array.isArray(unpack.vault_skipped) ? unpack.vault_skipped : [],
    blank,
    note: null,
  }
}
