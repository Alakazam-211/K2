import React from 'react'
import { Surface } from '@/components/ui'

// Context management stack (prd-context-hamburger-v1): always-on AGENTS.md is a
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
  { label: 'Project folder', path: './AGENTS.md', note: 'A real copy, written when you generate' },
  { label: 'Claude Code', path: './CLAUDE.md', note: 'Its bridge: Claude Code can’t read AGENTS.md itself' },
  { label: 'Gemini', path: './GEMINI.md' },
  { label: 'Cursor', path: './.cursor/rules/k2so.mdc' },
  { label: 'Goose', path: './.goosehints' },
  { label: 'GitHub Copilot', path: './.github/copilot-instructions.md' },
]

// The two LOADABLE skills — separate lane, NOT the canonical entrypoint.
// Loaded on demand from `.k2/skills/`, never compiled into AGENTS.md.
const LOADABLE_SKILLS: { label: string; path: string; blurb: string }[] = [
  { label: 'k2-cli', path: '.k2/skills/k2-cli/', blurb: 'How to use the k2 CLI: check in, report status, delegate and message.' },
  { label: 'k2-canonical-agents', path: '.k2/skills/k2-canonical-agents/', blurb: 'How to set up or undo the read-only links safely.' },
]

type Kind = 'always' | 'optional' | 'built' | 'link'

const KIND_DOT: Record<Kind, string> = {
  always: 'bg-[var(--color-accent)]',
  optional: 'bg-[var(--color-status-warn)]',
  built: 'bg-[var(--color-status-ok)]',
  link: 'bg-[var(--color-neutral)]',
}

function Dot({ kind }: { kind: Kind }): React.JSX.Element {
  return <span aria-hidden="true" className={`w-2 h-2 flex-shrink-0 rounded-[var(--radius-badge)] ${KIND_DOT[kind]}`} />
}

/** One box in the diagram: what it is, where it lives, why. */
function Box({ kind, title, path, note }: { kind: Kind; title: string; path?: string; note?: string }): React.JSX.Element {
  return (
    <Surface chamfer material="raised" role2="elevated" className="px-3 py-2 min-w-0">
      <div className="flex items-center gap-2 min-w-0">
        <Dot kind={kind} />
        <span className="type-row text-[var(--color-text-primary)] truncate">{title}</span>
      </div>
      {path && <div className="type-hint font-mono text-[var(--color-text-secondary)] mt-0.5 truncate" title={path}>{path}</div>}
      {note && <div className="type-hint text-[var(--color-text-secondary)] mt-0.5 leading-snug">{note}</div>}
    </Surface>
  )
}

function StageTitle({ children }: { children: React.ReactNode }): React.JSX.Element {
  return <div className="type-name text-[var(--color-text-primary)] mb-1">{children}</div>
}

function Arrow({ label }: { label: string }): React.JSX.Element {
  return (
    <div className="flex flex-col justify-center items-center text-[var(--color-text-secondary)]">
      <div className="type-row" aria-hidden="true">→</div>
      <div className="type-hint text-center">{label}</div>
    </div>
  )
}

export function AgentContextDiagram(): React.JSX.Element {
  return (
    <Surface chamfer material="tile" className="px-4 py-3 mb-4">
      <div className="flex items-center justify-between gap-4 flex-wrap mb-3">
        <h3 className="type-heading text-[var(--color-text-primary)]">How context reaches your agents</h3>
        <div className="flex items-center gap-3 flex-wrap type-hint text-[var(--color-text-secondary)]">
          <span className="flex items-center gap-1.5"><Dot kind="always" /> Always included</span>
          <span className="flex items-center gap-1.5"><Dot kind="optional" /> Optional</span>
          <span className="flex items-center gap-1.5"><Dot kind="built" /> Built by K2</span>
          <span className="flex items-center gap-1.5"><Dot kind="link" /> Read-only link</span>
        </div>
      </div>

      <div className="grid gap-3 items-stretch" style={{ gridTemplateColumns: 'minmax(0,1.1fr) auto minmax(0,0.95fr) auto minmax(0,1.15fr)' }}>
        <div className="flex flex-col gap-1.5">
          <StageTitle>What goes in</StageTitle>
          <Box kind="always" title="ROLE.md" path=".k2/agent/ROLE.md" note="The agent's role" />
          <Box kind="always" title="PROJECT.md" path=".k2/PROJECT.md" note="About the project" />
          <Box kind="optional" title="Optional layers" note="Wiki index, user docs and packs, in the order you set" />
          <Box kind="always" title="Tooling" note="A pointer to the k2 CLI, written by K2" />
          <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-snug">Edit these in a workspace&rsquo;s Context tab.</p>
        </div>

        <Arrow label="K2 builds" />

        <div className="flex flex-col gap-1.5 justify-center">
          <StageTitle>What K2 builds</StageTitle>
          <Box kind="built" title="AGENTS.md" path=".k2/AGENTS.md" note="The one source every tool reads from" />
          <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-snug">
            One file from what you&rsquo;ve turned on. Most AI tools read it as it is.
          </p>
        </div>

        <Arrow label="links" />

        <div className="flex flex-col gap-1.5">
          <StageTitle>What each tool sees</StageTitle>
          {HARNESS_MIRRORS.map((m) => (
            <Box key={m.path} kind="link" title={m.label} path={m.path} note={m.note} />
          ))}
        </div>
      </div>

      <div className="mt-3 pt-3 border-t border-[var(--color-border)]">
        <div className="type-name text-[var(--color-text-primary)]">Skills load when needed</div>
        <p className="type-hint text-[var(--color-text-secondary)] mt-0.5 mb-2">
          An agent opens these when a task calls for one. They aren&rsquo;t part of the shared file.
        </p>
        <div className="grid grid-cols-2 gap-1.5">
          {LOADABLE_SKILLS.map((s) => (
            <Surface key={s.path} chamfer material="raised" role2="elevated" className="px-3 py-2 min-w-0">
              <div className="type-row text-[var(--color-text-primary)] truncate">{s.label}</div>
              <div className="type-hint font-mono text-[var(--color-text-secondary)] truncate">{s.path}</div>
              <div className="type-hint text-[var(--color-text-secondary)] mt-0.5 leading-snug">{s.blurb}</div>
            </Surface>
          ))}
        </div>
      </div>

      <p className="mt-3 pt-3 border-t border-[var(--color-border)] type-hint text-[var(--color-text-secondary)] leading-snug">
        K2 always writes a copy in the project folder. The links for the other tools appear only
        when the workspace&rsquo;s Canonical Agent box is ticked.
      </p>
    </Surface>
  )
}
