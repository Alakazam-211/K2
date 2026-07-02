// Dev-only workspace-switch t0 mark — the shared clock for the
// pinned-chat retention win metric. `projects.ts` stamps it at the top
// of every workspace/project switch body; TerminalPane's
// `[v2-perf] stage=show_to_painted` line reads it to report
// switch→painted end-to-end (the number the retention feature exists to
// crush: ~0.5–1.5 s remote parked reattach → sub-frame portal move).
//
// Tiny standalone module (no imports) so TerminalPane can read the mark
// without pulling the projects store into kessel-term.

let lastMarkAt: number | null = null

/** Stamp "a workspace switch started now". Recorded unconditionally (a
 *  single number assignment); the CONSUMER (TerminalPane's perfLog) is
 *  the dev gate — this module deliberately avoids `import.meta.env`,
 *  which this repo's web tsconfig can't type. */
export function markWorkspaceSwitch(): void {
  lastMarkAt = performance.now()
}

/**
 * The most recent switch mark, or null when none exists or it is older
 * than `maxAgeMs` — a pane becoming visible minutes after the last
 * switch (tab click, settings close) must not report a bogus
 * since_switch_ms against a stale mark.
 */
export function readWorkspaceSwitchMark(maxAgeMs = 10_000): number | null {
  if (lastMarkAt === null) return null
  return performance.now() - lastMarkAt <= maxAgeMs ? lastMarkAt : null
}
