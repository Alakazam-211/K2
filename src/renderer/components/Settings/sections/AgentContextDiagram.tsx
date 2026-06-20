import React from 'react'

// Reworked for the AGENTS.md-canonical redesign (canonical-agents PRD §4 / §7
// / §11 + the AGENTS.md-canonical flow). Two stale depictions are corrected:
//   • The OLD diagram showed the composed `.k2/skills/k2so/SKILL.md` as the
//     canonical artifact CLAUDE.md symlinked to. That buried SKILL.md is NOT
//     canonical and is removed from the picture.
//   • The OLD diagram claimed K2 "fans out to every harness — 12 harnesses"
//     unconditionally. Mirroring is OPT-IN and per-harness.
//
// The new shape is a three-stage flow plus a separate skills lane:
//   1. AUTHORED  — you + the agent write `.k2/AGENT.md` (persona) and
//      `.k2/PROJECT.md` (project). Model A: the source of truth.
//   2. GENERATED — K2 compiles them into `.k2/AGENTS.md`, the canonical
//      entrypoint. AGENTS.md is the cross-tool standard read natively by
//      Pi / Hermes / Codex / +28 other harnesses.
//   3. MIRRORS   — CLAUDE.md / GEMINI.md / .cursor/rules are read-only
//      symlinks pointing at `.k2/AGENTS.md`. CLAUDE.md is the bridge for
//      Claude Code, which doesn't read AGENTS.md natively.
//
// Separate lane: the two LOADABLE skills (`k2-cli`, `k2-canonical-agents`)
// live under `.k2/skills/`, NOT in the canonical entrypoint — they're loaded
// on demand, not compiled into AGENTS.md.

// The read-only symlink mirrors of the generated `.k2/AGENTS.md` entrypoint.
// Per-harness + opt-in, not a blanket "12 harnesses". CLAUDE.md is called out
// as the bridge because Claude Code can't read AGENTS.md natively.
const HARNESS_MIRRORS: { label: string; path: string; note?: string }[] = [
  { label: 'Claude Code', path: './CLAUDE.md', note: 'bridge — Claude Code can’t read AGENTS.md natively' },
  { label: 'Gemini', path: './GEMINI.md' },
  { label: 'Cursor', path: './.cursor/rules/k2.mdc' },
  { label: 'Goose', path: './.goosehints' },
  { label: 'GitHub Copilot', path: './.github/copilot-instructions.md' },
]

// The two LOADABLE skills — separate lane, NOT the canonical entrypoint.
// Loaded on demand from `.k2/skills/`, never compiled into AGENTS.md.
const LOADABLE_SKILLS: { label: string; path: string; blurb: string }[] = [
  { label: 'k2-cli', path: '.k2/skills/k2-cli/', blurb: 'The k2 CLI verb surface — checkin, status, done, delegate, review, msg.' },
  { label: 'k2-canonical-agents', path: '.k2/skills/k2-canonical-agents/', blurb: 'How to set up / unwind the canonical harness mirrors safely.' },
]

export function AgentContextDiagram(): React.JSX.Element {
  return (
    <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/30 px-4 py-3 mb-4">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-[11px] font-semibold text-[var(--color-text-primary)]">
          Authored → generated canon → harness mirrors
        </h3>
        <div className="flex items-center gap-3 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-violet-400/50 bg-violet-400/10" /> authored</span>
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-sky-400/50 bg-sky-400/10" /> generated canon</span>
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-[var(--color-border)] bg-[var(--color-bg-elevated)]" /> mirror (symlink)</span>
        </div>
      </div>

      <div className="grid gap-2 items-stretch" style={{ gridTemplateColumns: 'minmax(0,1fr) auto minmax(0,0.95fr) auto minmax(0,1.15fr)' }}>
        {/* Stage 1: AUTHORED — Model A source of truth. */}
        <div className="flex flex-col gap-1.5">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">You + the agent author</div>
          <div className="border border-violet-400/50 bg-violet-400/10 px-2 py-2">
            <div className="text-[12px] font-semibold text-violet-200">AGENT.md</div>
            <div className="text-[9px] font-mono text-violet-200/60 mt-0.5 truncate">.k2/AGENT.md</div>
            <div className="text-[8px] text-violet-200/70 mt-0.5">persona</div>
          </div>
          <div className="border border-violet-400/40 bg-violet-400/5 px-2 py-2">
            <div className="text-[11px] font-medium text-violet-200/90">PROJECT.md</div>
            <div className="text-[9px] font-mono text-violet-200/50 mt-0.5 truncate">.k2/PROJECT.md</div>
            <div className="text-[8px] text-violet-200/60 mt-0.5">project</div>
          </div>
          <div className="text-[9px] text-[var(--color-text-muted)] italic mt-1 leading-snug">
            The source of truth (Model A). Edit these — everything downstream is derived.
          </div>
        </div>

        {/* Arrow: authored → generated. */}
        <div className="flex flex-col justify-center items-center text-[var(--color-text-muted)]">
          <div className="text-xs">→</div>
          <div className="text-[8px] uppercase tracking-wider mt-1 text-center leading-snug">K2<br />generates</div>
        </div>

        {/* Stage 2: GENERATED canonical entrypoint. */}
        <div className="flex flex-col gap-1.5 justify-center">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">Generated canon</div>
          <div className="border border-sky-400/50 bg-sky-400/10 px-2 py-2.5">
            <div className="flex items-center justify-between gap-2">
              <div className="text-[12px] font-semibold text-sky-200">AGENTS.md</div>
              <div className="text-[8px] uppercase tracking-wider px-1.5 py-0.5 bg-sky-400/20 text-sky-100 rounded-sm">canonical</div>
            </div>
            <div className="text-[9px] font-mono text-sky-200/60 mt-1 truncate">.k2/AGENTS.md</div>
          </div>
          <div className="text-[9px] text-[var(--color-text-muted)] italic mt-1 leading-snug">
            The cross-tool standard — read natively by Pi, Hermes, Codex &amp; 28+ others.
          </div>
        </div>

        {/* Arrow: generated → mirrors. */}
        <div className="flex flex-col justify-center items-center text-[var(--color-text-muted)]">
          <div className="text-xs">→</div>
          <div className="text-[8px] uppercase tracking-wider mt-1 text-center leading-snug">symlink<br />mirrors</div>
        </div>

        {/* Stage 3: read-only symlink mirrors. */}
        <div className="flex flex-col gap-1">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">Read-only mirrors</div>
          {HARNESS_MIRRORS.map((m) => (
            <div key={m.path} className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)] px-1.5 py-1" title={m.note ?? m.path}>
              <div className="flex items-baseline justify-between gap-2">
                <div className="text-[10px] text-[var(--color-text-secondary)] truncate">{m.label}</div>
                <div className="text-[8px] font-mono text-[var(--color-text-muted)] truncate">{m.path}</div>
              </div>
              {m.note ? <div className="text-[8px] text-[var(--color-text-muted)]/80 italic mt-0.5 leading-snug">{m.note}</div> : null}
            </div>
          ))}
        </div>
      </div>

      {/* Separate lane: the two LOADABLE skills (not the canonical entrypoint). */}
      <div className="mt-3 pt-2.5 border-t border-[var(--color-border)]">
        <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1.5">
          Loadable skills — separate lane, loaded on demand from <span className="font-mono">.k2/skills/</span> (NOT compiled into AGENTS.md)
        </div>
        <div className="grid grid-cols-2 gap-1.5">
          {LOADABLE_SKILLS.map((s) => (
            <div key={s.path} className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/50 px-2 py-1.5">
              <div className="flex items-baseline gap-2">
                <span className="text-[10px] font-medium text-[var(--color-text-secondary)]">{s.label}</span>
                <span className="text-[8px] font-mono text-[var(--color-text-muted)] truncate">{s.path}</span>
              </div>
              <div className="text-[8px] text-[var(--color-text-muted)] mt-0.5 leading-snug">{s.blurb}</div>
            </div>
          ))}
        </div>
      </div>

      {/* Footer: the two opt-in routes that produce the mirrors. */}
      <div className="mt-3 pt-2 border-t border-[var(--color-border)] text-[9px] text-[var(--color-text-muted)] leading-snug">
        Mirroring is <span className="text-[var(--color-text-secondary)] font-medium">opt-in and per-harness</span>.
        Run the <span className="text-[var(--color-text-secondary)]">K2 Canonical Agent</span> with an AI assistant
        for a safe, content-preserving merge of an existing project, or enable harness fan-out below for ongoing
        <span className="text-[var(--color-text-secondary)]"> symlinks</span>. Nothing is mirrored until you choose a route.
      </div>
    </div>
  )
}
