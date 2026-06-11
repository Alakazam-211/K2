import React from 'react'

// Reworked for the canonical-agents PRD (§4 / §7 / §11). The OLD diagram
// showed the composed SKILL.md as the canonical artifact and claimed K2
// "fans it out to every harness — 12 harnesses" unconditionally. Both are
// corrected here:
//   • THE canonical artifact is .k2so/agent/AGENT.md (Model A). The
//     per-harness files (CLAUDE.md, GEMINI.md, …) are MIRRORS of it.
//   • Mirroring is OPT-IN and per-harness — never an always-on 12-way
//     fan-out. Two routes: the K2 Canonical Agent skill (safe copies) or
//     the harness-fan-out checkbox (programmatic symlinks).

// The harness mirror targets K2 actually supports (from core —
// HARNESS_WORKSPACE_FILES + the extended probe list). Per-harness + opt-in,
// not a blanket "12 harnesses".
const HARNESS_MIRRORS: { label: string; path: string }[] = [
  { label: 'Claude Code', path: './CLAUDE.md' },
  { label: 'Gemini', path: './GEMINI.md' },
  { label: 'Agent.md (generic)', path: './AGENT.md' },
  { label: 'Goose', path: './.goosehints' },
  { label: 'Cursor', path: './.cursor/rules/k2so.mdc' },
  { label: 'Aider', path: './.aider.conf.yml (read:)' },
  { label: 'OpenCode', path: './.opencode/agent/k2so.md' },
  { label: 'Pi', path: './.pi/skills/k2so/SKILL.md' },
  { label: 'AGENTS.md (multi)', path: './AGENTS.md (marker)' },
  { label: 'GitHub Copilot', path: './.github/copilot-instructions.md (marker)' },
]

export function AgentContextDiagram(): React.JSX.Element {
  return (
    <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/30 px-4 py-3 mb-4">
      <div className="flex items-center justify-between mb-2">
        <h3 className="text-[11px] font-semibold text-[var(--color-text-primary)]">
          Canonical source → harness mirrors
        </h3>
        <div className="flex items-center gap-3 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-sky-400/50 bg-sky-400/10" /> canonical</span>
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-[var(--color-border)] bg-[var(--color-bg-elevated)]" /> mirror (opt-in)</span>
        </div>
      </div>

      <div className="grid gap-3 items-center" style={{ gridTemplateColumns: 'minmax(0,1fr) auto minmax(0,1.3fr)' }}>
        {/* Col 1: the single canonical source (Model A). */}
        <div className="flex flex-col gap-1.5">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">You + the agent author — Model A</div>
          <div className="border border-sky-400/50 bg-sky-400/10 px-2 py-2">
            <div className="flex items-center justify-between gap-2">
              <div className="text-[12px] font-semibold text-sky-200">AGENT.md</div>
              <div className="text-[8px] uppercase tracking-wider px-1.5 py-0.5 bg-sky-400/20 text-sky-100 rounded-sm">canonical</div>
            </div>
            <div className="text-[9px] font-mono text-sky-200/60 mt-1 truncate">.k2so/agent/AGENT.md</div>
          </div>
          <div className="border border-sky-400/30 bg-sky-400/5 px-2 py-1.5">
            <div className="text-[11px] font-medium text-sky-200/90">PROJECT.md</div>
            <div className="text-[9px] font-mono text-sky-200/50 mt-0.5 truncate">.k2so/PROJECT.md</div>
          </div>
          <div className="text-[9px] text-[var(--color-text-muted)] italic mt-1 leading-snug">
            The source of truth. The harness files are derived FROM here — never the reverse.
          </div>
        </div>

        {/* Arrow + route legend. */}
        <div className="flex flex-col justify-center items-center text-[var(--color-text-muted)]">
          <div className="text-xs">→</div>
          <div className="text-[8px] uppercase tracking-wider mt-1 text-center leading-snug">
            opt-in<br />copy or symlink
          </div>
        </div>

        {/* Col 2: per-harness mirrors. */}
        <div className="flex flex-col gap-1">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">Mirrors — per-harness, opt-in</div>
          <div className="grid grid-cols-2 gap-1">
            {HARNESS_MIRRORS.map((m) => (
              <div key={m.path} className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)] px-1.5 py-1" title={m.path}>
                <div className="text-[10px] text-[var(--color-text-secondary)] truncate">{m.label}</div>
                <div className="text-[8px] font-mono text-[var(--color-text-muted)] truncate">{m.path}</div>
              </div>
            ))}
          </div>
        </div>
      </div>

      {/* Footer: the two opt-in routes. */}
      <div className="mt-3 pt-2 border-t border-[var(--color-border)] text-[9px] text-[var(--color-text-muted)] leading-snug">
        Mirroring is <span className="text-[var(--color-text-secondary)] font-medium">opt-in and per-harness</span>.
        Run the <span className="text-[var(--color-text-secondary)]">K2 Canonical Agent</span> skill for safe,
        byte-reversible <span className="text-[var(--color-text-secondary)]">copies</span>, or enable harness
        fan-out for ongoing <span className="text-[var(--color-text-secondary)]">symlinks</span>. Nothing is
        mirrored until you choose a route.
      </div>
    </div>
  )
}
