import { useCallback, useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'
import { AIFileEditor } from '@/components/AIFileEditor/AIFileEditor'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { daemonCliGet } from '@/lib/daemon-cli'
import { CANONICAL_SETUP_SEED, CANONICAL_MANAGE_SEED } from './canonicalAgentSeeds'
import type { HarnessProbe } from './canonicalState'
import { harnessStateLabel } from './canonicalState'

// ── Manifest shapes (mirror crates/k2so-core/src/workspace/canonical.rs) ──
interface ManifestEntry {
  relative_path: string
  harness: string
  action: string
  backup_relative_path: string | null
  pre_content_hash: string | null
}
interface SetupManifest {
  timestamp: string
  entries: ManifestEntry[]
}

const ACTION_BADGE: Record<string, string> = {
  created: 'text-emerald-300 bg-emerald-500/10 border-emerald-500/30',
  backed_up_then_wrote: 'text-amber-300 bg-amber-500/10 border-amber-500/30',
  merge: 'text-sky-300 bg-sky-500/10 border-sky-500/30',
  marker_injected: 'text-purple-300 bg-purple-500/10 border-purple-500/30',
  skip: 'text-[var(--color-text-muted)] bg-[var(--color-bg-elevated)] border-[var(--color-border)]',
}

function actionLabel(action: string): string {
  switch (action) {
    case 'created':
      return 'Create'
    case 'backed_up_then_wrote':
      return 'Back up + write'
    case 'merge':
      return 'Merge'
    case 'marker_injected':
      return 'Marker block'
    case 'skip':
      return 'Skip (already managed)'
    default:
      return action
  }
}

/**
 * The K2 Canonical Agent ceremony modal (canonical-agents PRD §9.2).
 *
 * NOT a single-file editor — it is the agent terminal on one side + a
 * STRUCTURED plan/manifest renderer on the other (per-harness action table
 * + the backup manifest). The plan is read from `.k2so/.canonical-setup/
 * plan.md`; the manifest from the latest `.k2so/backups/<ts>/manifest.json`.
 * `disableSessionResume` is set so the one-shot ceremony always launches a
 * fresh agent session.
 */
export function CanonicalAgentModal({
  projectPath,
  projectName,
  mode,
  onClose,
}: {
  projectPath: string
  projectName: string
  mode: 'setup' | 'manage'
  onClose: () => void
}): React.JSX.Element {
  const planPath = `${projectPath}/.k2so/.canonical-setup/plan.md`
  const watchDir = `${projectPath}/.k2so`

  const [planContent, setPlanContent] = useState('')
  const [probes, setProbes] = useState<HarnessProbe[]>([])
  const [manifest, setManifest] = useState<SetupManifest | null>(null)

  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const agentCommand = useMemo(() => {
    const preset = presets.find((p) => p.id === defaultAgent) || presets.find((p) => p.enabled)
    if (!preset) return null
    return parseCommand(preset.command)
  }, [defaultAgent, presets])

  // Detect per-harness state up front (and on manual refresh).
  const refreshState = useCallback(async () => {
    try {
      const next = await invoke<HarnessProbe[]>('k2so_detect_canonical_state', { projectPath })
      setProbes(next)
    } catch (err) {
      console.warn('[canonical-modal] detect_canonical_state failed:', err)
    }
  }, [projectPath])

  // Read the latest manifest.json under .k2so/backups/<ts>/.
  const refreshManifest = useCallback(async () => {
    try {
      const entries = await invoke<{ name: string; path: string; isDirectory: boolean }[]>(
        'fs_read_dir',
        { path: `${projectPath}/.k2so/backups` },
      )
      const dirs = entries.filter((e) => e.isDirectory).map((e) => e.name).sort()
      const latest = dirs[dirs.length - 1]
      if (!latest) {
        setManifest(null)
        return
      }
      const r = await daemonCliGet<{ content: string }>('fs/read-file', {
        path: `${projectPath}/.k2so/backups/${latest}/manifest.json`,
      })
      setManifest(JSON.parse(r.content) as SetupManifest)
    } catch {
      setManifest(null)
    }
  }, [projectPath])

  useEffect(() => {
    refreshState()
    refreshManifest()
  }, [refreshState, refreshManifest])

  // The watcher re-reads plan.md as the agent writes it; also re-poll the
  // structured state so the table tracks what the agent just did.
  const handleFileChange = useCallback(
    (content: string, path?: string) => {
      if (!path || path === planPath) setPlanContent(content)
      void refreshState()
      void refreshManifest()
    },
    [planPath, refreshState, refreshManifest],
  )

  const seedSystemPrompt = useMemo(
    () =>
      [
        `You are the K2 Canonical Agent for the workspace "${projectName}".`,
        ``,
        `Source of truth (Model A): .k2so/agent/AGENT.md + .k2so/PROJECT.md. The per-harness`,
        `files (CLAUDE.md, GEMINI.md, …) are MIRRORS derived from them — never the reverse.`,
        ``,
        `All destructive file mutation goes through the deterministic core (backup + atomic`,
        `write + manifest). You author merged text; the core persists it. Default is dry-run;`,
        `writing requires explicit confirmation. Write the plan to`,
        `.k2so/.canonical-setup/plan.md so the user sees it rendered alongside this terminal.`,
        ``,
        mode === 'setup' ? CANONICAL_SETUP_SEED : CANONICAL_MANAGE_SEED,
      ].join('\n'),
    [projectName, mode],
  )

  const terminalArgs = useMemo(() => {
    if (!agentCommand) return undefined
    const baseArgs = [...agentCommand.args]
    if (agentCommand.command === 'claude') {
      return [
        ...baseArgs,
        '--append-system-prompt',
        seedSystemPrompt,
        mode === 'setup' ? CANONICAL_SETUP_SEED : CANONICAL_MANAGE_SEED,
      ]
    }
    return baseArgs
  }, [agentCommand, seedSystemPrompt, mode])

  // Plan rows preferred from the live manifest (richer + structured); the
  // detected per-harness state is the fallback so the table is populated
  // even before the agent has produced a plan.
  const planRows = useMemo(() => {
    if (manifest && manifest.entries.length > 0) {
      return manifest.entries.map((e) => ({
        path: e.relative_path,
        harness: e.harness || '(canonical AGENT.md)',
        action: e.action,
        backup: e.backup_relative_path,
      }))
    }
    return probes.map((p) => ({
      path: p.relative_path,
      harness: p.label,
      action: p.state.kind === 'unified' ? 'skip' : p.state.kind === 'unmanaged' ? 'merge' : 'skip',
      backup: null as string | null,
    }))
  }, [manifest, probes])

  return (
    <AIFileEditor
      filePath={planPath}
      watchDir={watchDir}
      cwd={watchDir}
      command={agentCommand?.command}
      args={terminalArgs}
      disableSessionResume
      title={`K2 Canonical Agent — ${projectName} (${mode === 'setup' ? 'Set up' : 'Manage / Undo'})`}
      instructions={
        mode === 'setup'
          ? 'Unify this workspace’s AI-harness files safely. The agent diagnoses per-harness state, proposes a dry-run plan, and waits for your confirmation before any write.'
          : 'Review the current canonical state and the exact undo. The agent runs a manifest-driven unwind only for the harnesses you confirm.'
      }
      warningText="The deterministic core is the only thing that writes files here — every original is backed up first and the change is byte-reversible."
      onFileChange={handleFileChange}
      onClose={onClose}
      preview={
        <div className="h-full flex flex-col overflow-auto">
          {/* Per-harness plan/manifest table — same card/border idiom as
              the Canonical Agent Flow settings section (PRD §9.2 / §11). */}
          <div className="px-4 py-3 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="text-xs font-medium text-[var(--color-text-primary)]">
              Per-harness plan
            </div>
            <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
              {manifest
                ? `From manifest ${manifest.timestamp}`
                : 'Detected state (no plan written yet)'}
            </div>
          </div>
          <div className="p-3 space-y-1.5">
            {planRows.length === 0 ? (
              <p className="text-[11px] text-[var(--color-text-muted)] italic">
                No harness files detected yet.
              </p>
            ) : (
              planRows.map((row) => (
                <div
                  key={`${row.path}:${row.harness}`}
                  className="flex items-start gap-2 py-1.5 px-2 border border-[var(--color-border)] bg-[var(--color-bg-elevated)]"
                >
                  <div
                    className={`px-1.5 py-0.5 text-[9px] uppercase tracking-wider border whitespace-nowrap ${
                      ACTION_BADGE[row.action] ?? ACTION_BADGE.skip
                    }`}
                  >
                    {actionLabel(row.action)}
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="text-[11px] font-mono text-[var(--color-text-primary)] truncate">
                      {row.path}
                    </div>
                    <div className="text-[10px] text-[var(--color-text-muted)] leading-snug">
                      {row.harness}
                      {row.backup ? (
                        <>
                          {' · backup → '}
                          <span className="font-mono">{row.backup}</span>
                        </>
                      ) : null}
                    </div>
                  </div>
                </div>
              ))
            )}
          </div>

          {/* Detected state summary (independent of the plan). */}
          {probes.length > 0 ? (
            <div className="px-4 pt-2 pb-1 border-t border-[var(--color-border)]">
              <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1.5">
                Current per-harness state
              </div>
              <div className="flex flex-col gap-1">
                {probes.map((p) => (
                  <div key={p.relative_path} className="flex items-center justify-between gap-2 text-[10px]">
                    <span className="font-mono text-[var(--color-text-secondary)] truncate">
                      {p.relative_path}
                    </span>
                    <span className="text-[var(--color-text-muted)] whitespace-nowrap">
                      {harnessStateLabel(p.state)}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          ) : null}

          {/* The agent's prose plan (plan.md), rendered as markdown. */}
          {planContent ? (
            <div className="flex-1 overflow-auto p-4 border-t border-[var(--color-border)]">
              <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-2">
                plan.md
              </div>
              <div className="markdown-content text-[11px]">
                <Markdown remarkPlugins={[remarkGfm]}>{planContent}</Markdown>
              </div>
            </div>
          ) : null}
        </div>
      }
    />
  )
}
