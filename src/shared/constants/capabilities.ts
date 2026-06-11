// ── Workspace capability state definitions ───────────────────────────
// Shared across WorkspaceStatesSection, ProjectDetail, and
// AdaptiveHeartbeatConfig — anywhere we render capability pickers or
// display the current capability posture for a workspace.

export const CAP_STATES = ['auto', 'gated', 'off'] as const

export type CapState = (typeof CAP_STATES)[number]

export const CAP_LABELS: Record<string, string> = {
  auto: 'Auto',
  gated: 'Gated',
  off: 'Off',
}

export const CAP_COLORS: Record<string, string> = {
  auto: 'text-green-400',
  gated: 'text-amber-400',
  off: 'text-[var(--color-text-muted)]',
}

export const CAPABILITIES = [
  { key: 'capFeatures' as const, label: 'Features', desc: 'New functionality and enhancements' },
  { key: 'capIssues' as const, label: 'Issues', desc: 'Bug fixes from submitted issues' },
  { key: 'capCrashes' as const, label: 'Crashes', desc: 'Automatic crash report fixes' },
  { key: 'capSecurity' as const, label: 'Security', desc: 'Automatic security patches' },
  { key: 'capAudits' as const, label: 'Audits', desc: 'Scheduled code reviews' },
]

// Shape of a state row from `states_list`. Used in Settings → Workspace
// States (the editor), and in Settings → Workspaces (StateSelector +
// AdaptiveHeartbeatConfig) to show and change the current workspace state.
export interface StateData {
  id: string
  name: string
  description: string | null
  isBuiltIn: number
  capFeatures: string
  capIssues: string
  capCrashes: string
  capSecurity: string
  capAudits: string
  heartbeat: number
  sortOrder: number
}
