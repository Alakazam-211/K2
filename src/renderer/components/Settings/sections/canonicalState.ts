// TypeScript mirror of the canonical-state types returned by the
// `k2so_detect_canonical_state` Tauri command (canonical-agents PRD §5.2).
// Source of truth: crates/k2so-core/src/workspace/canonical.rs
// (HarnessProbe / HarnessFileState / UnifiedForm, serde tag = "kind").

export type UnifiedForm = 'copy' | 'symlink'

export type HarnessFileState =
  | { kind: 'unified'; form: UnifiedForm }
  | { kind: 'unmanaged' }
  | { kind: 'skipped' }

export interface HarnessProbe {
  relative_path: string
  label: string
  state: HarnessFileState
}

/** Short human label for a single harness file's state. */
export function harnessStateLabel(state: HarnessFileState): string {
  switch (state.kind) {
    case 'unified':
      return state.form === 'copy' ? 'Canonical (copy)' : 'Canonical (symlink)'
    case 'unmanaged':
      return 'Unmanaged'
    case 'skipped':
      return 'Not present'
  }
}

/**
 * Whether ANY harness file is canonicalized — drives the canonical button
 * label: at least one Unified → the user has set things up, so the button
 * is "Manage / Undo"; otherwise "Set up …" (PRD §9.3).
 */
export function anyHarnessUnified(probes: HarnessProbe[]): boolean {
  return probes.some((p) => p.state.kind === 'unified')
}
