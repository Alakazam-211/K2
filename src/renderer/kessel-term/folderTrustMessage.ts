/**
 * Folder trust (prd-security-sweep-0464-v1 SS11, review round 2 Low): a
 * spawn the daemon refuses with 409 `folder_trust_pending` /
 * `folder_trust_denied` shows the refusal's own sentence in the pane, not
 * the raw JSON body. `null` for any other body.
 */
export function folderTrustMessage(status: number, body: string): string | null {
  if (status !== 409) return null
  let v: unknown
  try {
    v = JSON.parse(body)
  } catch {
    return null
  }
  if (!v || typeof v !== 'object') return null
  const r = v as { error?: unknown; message?: unknown; decide?: unknown }
  if (r.error !== 'folder_trust_pending' && r.error !== 'folder_trust_denied') return null
  const message = typeof r.message === 'string' && r.message.trim() ? r.message.trim() : null
  if (message) return message
  const decide = typeof r.decide === 'string' ? r.decide : ''
  return r.error === 'folder_trust_denied'
    ? `K2 won't start programs in this folder: the owner chose "Don't trust".${decide ? ` ${decide}` : ''}`
    : `Waiting for the owner to trust this folder.${decide ? ` Decide: ${decide}` : ''}`
}
