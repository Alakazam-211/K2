// An API-triggered cell session in this workspace, from the daemon's sandbox
// index; clicking re-launches it in its sandbox (audit-resume) as its orange
// tab. The daemon says whether a sandbox was really in force (`sandboxed`)
// and whether this computer can re-launch it (`reopenable`); "Sandboxed"
// shows only on `sandboxed: true` (0.46.17). An older daemon sends neither.
export interface SandboxChat {
  sessionId: string
  title: string
  timestamp: number
  messageCount: number
  sandboxed?: boolean
  reopenable?: boolean
}

/** The section header and a row's second line, from what the daemon reports. */
export function sandboxChatLabels(rows: SandboxChat[]): { header: string; line: (s: SandboxChat) => string } {
  return {
    header: rows.length > 0 && rows.every((s) => s.sandboxed === true) ? 'Sandboxed' : 'Cell sessions',
    line: (s) =>
      s.sandboxed === false
        ? 'Unsandboxed: runs as you'
        : s.sandboxed !== true
          ? 'sandbox not confirmed by this server'
          : s.reopenable === false
            ? "sandbox · this computer can't re-launch it"
            : 'sandbox · click to re-launch',
  }
}
