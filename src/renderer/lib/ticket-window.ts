// Tickets "Open in window": the ticket a window was opened for. Same two
// sources as the Focus window (App.tsx parseFocusProjectId): the URL hash
// `#ticket=<id>` (dev, external URL) or the window label
// `window-ticket-<id>` (production, where the fragment may not survive).
// Window creation is the Tauri command `window_open_ticket` (menu.rs).

const ID_RE = /^[A-Za-z0-9-]{1,64}$/

export function ticketIdFromHash(hash: string): string | null {
  const m = hash.match(/^#ticket=(.+)$/)
  if (!m) return null
  const id = decodeURIComponent(m[1])
  return ID_RE.test(id) ? id : null
}

export function ticketIdFromLabel(label: string): string | null {
  const m = label.match(/^window-ticket-(.+)$/)
  if (!m) return null
  return ID_RE.test(m[1]) ? m[1] : null
}

export function parseTicketWindowId(hash: string, label: string | null): string | null {
  return ticketIdFromHash(hash) ?? (label ? ticketIdFromLabel(label) : null)
}
