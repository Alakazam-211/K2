// Connected Agents rows (Workspace panel): parse GET
// `/cli/connections?action=list` and decide what a federated row paints.
// Kept apart from the component so tests needn't load the store graph.

import { remoteConnectionGoneNote } from '@/lib/federation-gone'

/** One row from GET `/cli/connections?action=list` — present edges only. */
export type ConnListRow = {
  remote?: boolean
  projectId?: string
  projectName?: string
  path?: string
  reachable?: boolean
  address?: string
  host?: string
  agent?: string
  /** 0159: the bound workspace left its server; sends fail until re-added. */
  remoteWorkspaceGone?: boolean
}

/** What a federated row paints: a gone one (0159) is muted and says why. */
export function remoteConnRowView(row: ConnListRow): { title: string; detail: string; gone: boolean } {
  const title = row.agent || row.address || 'Remote agent'
  const host = row.host || row.address || ''
  if (row.remoteWorkspaceGone === true) {
    return { title, detail: remoteConnectionGoneNote(host), gone: true }
  }
  return { title, detail: `federated · ${host || 'federated'}`, gone: false }
}

export function parseConnectionRows(body: unknown): ConnListRow[] {
  let value: unknown = body
  if (typeof value === 'string') {
    try {
      value = JSON.parse(value)
    } catch {
      return []
    }
  }
  if (!value || typeof value !== 'object') return []
  const rows = (value as { connections?: unknown }).connections
  return Array.isArray(rows) ? (rows as ConnListRow[]) : []
}
