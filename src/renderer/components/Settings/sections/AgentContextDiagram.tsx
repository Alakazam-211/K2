import React from 'react'

// Context hamburger (prd-context-hamburger-v1): always-on AGENTS.md is a
// stack of markdown sources — pinned (AGENT + PROJECT + Tooling) plus
// optional ordered layers — composed server-side. Harness files stay
// read-only symlink mirrors of the generated `.k2/AGENTS.md`.
//
// Loadable skills (`.k2/skills/`) remain a separate lane: on-demand, not
// auto-compiled into AGENTS.md.

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
          Context stack → generated canon → harness mirrors
        </h3>
        <div className="flex items-center gap-3 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-violet-400/50 bg-violet-400/10" /> pinned</span>
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-amber-400/50 bg-amber-400/10" /> optional layers</span>
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-sky-400/50 bg-sky-400/10" /> generated canon</span>
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-[var(--color-border)] bg-[var(--color-bg-elevated)]" /> mirror (symlink)</span>
        </div>
      </div>

      <div className="grid gap-2 items-stretch" style={{ gridTemplateColumns: 'minmax(0,1.1fr) auto minmax(0,0.95fr) auto minmax(0,1.15fr)' }}>
        {/* Stage 1: CONTEXT STACK — pinned + optional layers. */}
        <div className="flex flex-col gap-1.5">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">Context stack (hamburger)</div>
          <div className="border border-violet-400/50 bg-violet-400/10 px-2 py-1.5">
            <div className="text-[11px] font-semibold text-violet-200">AGENT.md</div>
            <div className="text-[8px] font-mono text-violet-200/60 mt-0.5 truncate">.k2/agent/AGENT.md</div>
            <div className="text-[8px] text-violet-200/70">pinned · persona</div>
          </div>
          <div className="border border-violet-400/40 bg-violet-400/5 px-2 py-1.5">
            <div className="text-[11px] font-medium text-violet-200/90">PROJECT.md</div>
            <div className="text-[8px] font-mono text-violet-200/50 mt-0.5 truncate">.k2/PROJECT.md</div>
            <div className="text-[8px] text-violet-200/60">pinned · project</div>
          </div>
          <div className="border border-amber-400/50 bg-amber-400/10 px-2 py-1.5">
            <div className="text-[11px] font-medium text-amber-100/90">Optional layers</div>
            <div className="text-[8px] font-mono text-amber-100/50 mt-0.5 truncate">wiki index · user docs · packs</div>
            <div className="text-[8px] text-amber-100/70">ordered · toggleable</div>
          </div>
          <div className="border border-violet-400/30 bg-violet-400/5 px-2 py-1.5">
            <div className="text-[10px] font-medium text-violet-200/80">Tooling</div>
            <div className="text-[8px] text-violet-200/50">pinned · generated k2-cli pointer</div>
          </div>
          <div className="text-[9px] text-[var(--color-text-muted)] italic mt-1 leading-snug">
            Edit the stack in Workspace Settings or <span className="font-mono not-italic">k2 agent context</span>. Paths only — compose reads files at regen.
          </div>
        </div>

        {/* Arrow: stack → generated. */}
        <div className="flex flex-col justify-center items-center text-[var(--color-text-muted)]">
          <div className="text-xs">→</div>
          <div className="text-[8px] uppercase tracking-wider mt-1 text-center leading-snug">K2<br />composes</div>
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
            One composed entrypoint — pinned + enabled optional layers. Read natively by Pi, Hermes, Codex &amp; 28+ others.
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

      {/* Footer: stack editor + opt-in mirrors. */}
      <div className="mt-3 pt-2 border-t border-[var(--color-border)] text-[9px] text-[var(--color-text-muted)] leading-snug">
        Manage always-on context via the per-workspace{' '}
        <span className="text-[var(--color-text-secondary)] font-medium">stack editor</span> or{' '}
        <span className="font-mono text-[var(--color-text-secondary)]">k2 agent context</span>.
        Mirroring is <span className="text-[var(--color-text-secondary)] font-medium">opt-in and per-harness</span>
        {' '}— harness files stay <span className="text-[var(--color-text-secondary)]">symlinks</span> to AGENTS.md.
      </div>
    </div>
  )
}
