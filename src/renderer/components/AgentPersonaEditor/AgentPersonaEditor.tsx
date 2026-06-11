import { useState, useCallback, useEffect, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { AIFileEditor } from '../AIFileEditor/AIFileEditor'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'
import { CodeEditor } from '../FileViewerPane/CodeEditor'

// ── Helpers ─────────────────────────────────────────────────────────────

/** Strip YAML frontmatter (--- delimited) for clean markdown preview */
function stripFrontmatter(content: string): string {
  if (content.startsWith('---')) {
    const end = content.indexOf('---', 3)
    if (end !== -1) return content.slice(end + 3).trim()
  }
  return content.trim()
}

// ── Types ──────────────────────────────────────────────────────────────

interface EditorContext {
  agentName: string
  role: string
  agentType: string
  isCoordinator: boolean
  agentMd: string
  agentMdPath: string
  agentDir: string
}

interface AgentPersonaEditorProps {
  agentName: string
  projectPath: string
  onClose: () => void
}

// ── Component ──────────────────────────────────────────────────────────
//
// Single-file editor for the agent's AGENT.md (their persona / role /
// standing orders). Pre-0.36.9 this had four tabs (Profile, Wake-up,
// Agent CLAUDE.md, Workspace CLAUDE.md) that mixed source files with
// derived files — leading to confusion about which file was the truth.
//
// AGENT.md is the only source the user should be editing here:
//   - WAKEUP.md is edited from Settings → Heartbeats (per-heartbeat
//     wakeup files, with their own AIFileEditor surface)
//   - CLAUDE.md (agent + workspace) are derived from AGENT.md +
//     PROJECT.md via the regen pipeline; treating them as edit
//     surfaces just invited overwrites.
//
// Same shape as the Wakeup editor: one source file, save → close →
// regen pipeline runs → harness files update.

export function AgentPersonaEditor({ agentName, projectPath, onClose }: AgentPersonaEditorProps): React.JSX.Element {
  const [agentMdPath, setAgentMdPath] = useState<string | null>(null)
  const [watchDir, setWatchDir] = useState<string | null>(null)
  const [agentContent, setAgentContent] = useState<string>('')
  const [context, setContext] = useState<EditorContext | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [previewMode, setPreviewMode] = useState<'preview' | 'edit'>('preview')
  const [previewScale, setPreviewScale] = useState(100)
  const cssScale = Math.round(previewScale * 0.7)

  // Resolve the user's default AI agent command
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const agentCommand = useMemo(() => {
    const preset = presets.find((p) => p.id === defaultAgent) || presets.find((p) => p.enabled)
    if (!preset) return null
    return parseCommand(preset.command)
  }, [defaultAgent, presets])

  // Initialize: fetch agent context + CLAUDE.md preview
  useEffect(() => {
    const init = async () => {
      try {
        const ctx = await invoke<EditorContext>('k2so_agents_get_editor_context', {
          projectPath,
          agentName,
        })
        setContext(ctx)
        setAgentContent(ctx.agentMd)
        setAgentMdPath(ctx.agentMdPath)
        setWatchDir(ctx.agentDir)
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err)
        console.error('[agent-editor] Init failed:', msg)
        setError(msg)
      }
    }
    init()
  }, [projectPath, agentName])

  const handleFileChange = useCallback((content: string, _path?: string) => {
    setAgentContent(content)
  }, [])

  // Manual refresh: re-read AGENT.md directly
  const handleManualRefresh = useCallback(async () => {
    if (!agentMdPath) return
    try {
      const result = await daemonCliGet<{ content: string }>('fs/read-file', { path: agentMdPath })
      handleFileChange(result.content)
    } catch (err) {
      console.error('[agent-editor] Manual refresh failed:', err)
    }
  }, [agentMdPath, handleFileChange])

  // On close: backup AGENT.md to the agent-backups dir, then run the
  // workspace SKILL regen so AGENT.md edits propagate into every CLI
  // harness file before the editor closes. Same shape as the Wakeup
  // editor's flow.
  const handleClose = useCallback(async () => {
    try {
      if (agentMdPath) {
        const result = await daemonCliGet<{ content: string }>('fs/read-file', { path: agentMdPath })
        await daemonCliPost('agents/save-agent-md', {
          project_path: projectPath,
          agent_name: agentName,
          content: result.content,
        })
      }
    } catch (err) {
      console.error('[agent-editor] Failed to save on close:', err)
    }
    try {
      await daemonCliPost('agents/regenerate-workspace-skill', { project_path: projectPath })
    } catch (err) {
      console.warn('[agent-editor] regen on close failed:', err)
    }
    onClose()
  }, [projectPath, agentName, agentMdPath, onClose])

  // Build the system prompt for the AI assistant. AGENT.md is the
  // single source of truth — CLAUDE.md / AGENTS.md / GEMINI.md and
  // every other harness file are *compiled outputs*. Editing AGENT.md
  // and letting the regen pipeline run on close is the only way to
  // keep them coherent. WAKEUP.md is owned by the Heartbeats editor.
  const agentPrompt = useMemo(() => {
    if (!context) return ''
    const isCustom = context.agentType === 'custom'
    const isK2SO = context.agentType === 'k2so'

    const typeLabel = isK2SO ? 'K2 Agent' : isCustom ? 'Custom Agent' : context.isCoordinator ? 'Workspace Manager' : 'Agent Template'

    const typeGuidance = isK2SO
      ? [
          `This is the K2 Agent — the top-level planner and orchestrator for this workspace.`,
          ``,
          `The default K2 agent knowledge (CLI tools, workflow docs, work queue structure)`,
          `is auto-injected into the compiled SKILL at launch. AGENT.md should ADD project-specific`,
          `context on top of that — not replace the defaults.`,
          ``,
          `Help the user define:`,
          `• Work Sources — where new work comes from (\`gh issue list\`, \`linear issue list\`, etc.)`,
          `• Integration Commands — CLI tools the agent should use to check for work or report status`,
          `• Constraints — hours of operation, cost limits, repos/branches that are off-limits`,
        ].join('\n')
      : isCustom
        ? [
            `This is a Custom Agent — it runs purely from AGENT.md with no K2 infrastructure injected.`,
            `The body of AGENT.md IS the agent's entire system prompt when it wakes up on the heartbeat.`,
            `Focus on: what software it operates, what it does on each wake, tools/APIs it uses, constraints.`,
          ].join('\n')
        : [
            `This agent runs within K2. The following are auto-injected into the compiled SKILL`,
            `(don't duplicate them in AGENT.md):`,
            `• K2 CLI tools reference`,
            `• Workflow docs (lead agent vs sub-agent patterns)`,
            `• Work queue structure (inbox/active/done folders)`,
            `• Other agents list (for delegation awareness)`,
            ``,
            `Focus AGENT.md on what makes this agent unique beyond the standard K2 setup.`,
          ].join('\n')

    return [
      `You're helping the user configure an AI agent's persona/role.`,
      ``,
      `Agent: "${context.agentName}"`,
      `Role: ${context.role}`,
      `Type: ${typeLabel}`,
      ``,
      `## The only file you should edit here: AGENT.md`,
      ``,
      `Path: \`${context.agentMdPath}\``,
      ``,
      `AGENT.md is the source of truth for this agent's identity. It has two parts:`,
      `• Frontmatter (between --- delimiters): name, role, type — read by the K2 system`,
      `• Body (below frontmatter): persona, standing orders, tools, constraints, personality`,
      ``,
      `K2 compiles AGENT.md into the agent's SKILL.md and into every CLI harness file`,
      `(CLAUDE.md, AGENTS.md, GEMINI.md, .cursor/rules/k2so.mdc, .goosehints, etc.) automatically`,
      `when this editor closes. **Do not edit those compiled files directly** — your changes`,
      `will be overwritten. If something is wrong with a compiled file, fix it in AGENT.md.`,
      ``,
      `## Other source files you might want to mention to the user`,
      ``,
      `• **WAKEUP.md** — operational wake-up instructions for this agent's heartbeat.`,
      `  Edited from Settings → Heartbeats, not here. If the user asks "what does the agent`,
      `  do on each wake?", point them at the Heartbeats editor.`,
      ``,
      `• **PROJECT.md** — shared workspace knowledge (tech stack, conventions, key paths)`,
      `  injected into every agent's SKILL. Edited from Settings → Workspace Knowledge.`,
      `  If the user mentions project-wide info, suggest PROJECT.md instead of stuffing it`,
      `  into this one agent's AGENT.md.`,
      `  Path: \`${projectPath}/.k2so/PROJECT.md\``,
      ``,
      `## Key sections to help the user configure in AGENT.md`,
      ``,
      `• **Standing Orders** — persistent directives the agent follows every wake (e.g.,`,
      `  "Check CI status", "Review open PRs older than 24h", "Scan for new issues").`,
      `  These are ongoing responsibilities, distinct from one-off inbox items.`,
      ``,
      `Current AGENT.md contents:`,
      context.agentMd,
      ``,
      typeGuidance,
      ``,
      `Suggest edits, then ask before writing — AGENT.md affects every future wake.`,
    ].join('\n')
  }, [context, projectPath])

  const terminalCommand = agentCommand?.command
  const terminalArgs = useMemo(() => {
    if (!agentCommand || !context) return undefined
    const baseArgs = [...agentCommand.args]
    const isClaude = agentCommand.command === 'claude'
    if (isClaude) {
      return [
        ...baseArgs,
        '--append-system-prompt', agentPrompt,
        `Open and read AGENT.md in the current directory. This single file defines the agent "${context.agentName}" (${context.role}). The compiled SKILL.md and all harness files are regenerated from it on close — do not edit them directly. Start by asking what the user wants this agent to do.`,
      ]
    }
    return baseArgs
  }, [agentCommand, agentPrompt, context])

  // ── Conditional returns (after all hooks) ────────────────────────────

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-64 gap-3">
        <p className="text-xs text-red-400">Failed to initialize agent editor: {error}</p>
        <button onClick={onClose} className="text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer no-drag">&larr; Back</button>
      </div>
    )
  }

  if (!agentMdPath || !watchDir || !context) {
    return (
      <div className="flex items-center justify-center h-64 text-xs text-[var(--color-text-muted)]">
        Setting up agent editor...
      </div>
    )
  }

  return (
    <AIFileEditor
      filePath={agentMdPath}
      showTabs={false}
      watchDir={watchDir}
      cwd={watchDir}
      command={terminalCommand}
      args={terminalArgs}
      title={`Agent: ${agentName}`}
      instructions={undefined}
      warningText="This agent has full system access when running."
      onFileChange={handleFileChange}
      onClose={handleClose}
      onManualRefresh={handleManualRefresh}
      preview={
        <div className="h-full flex flex-col">
          {/* Header */}
          <div className="flex items-center justify-between px-3 py-1.5 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="text-[10px] font-medium text-[var(--color-text-muted)]">
              <span className="text-[var(--color-text-primary)]">AGENT.md</span>
              <span className="mx-1.5">·</span>
              Source — compiled into SKILL.md and every CLI harness file on close
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {previewMode === 'preview' && (
                <div className="flex items-center gap-0.5">
                  <button
                    onClick={() => setPreviewScale((s) => Math.max(50, s - 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                    title="Zoom out"
                  >
                    −
                  </button>
                  <span className="text-[9px] tabular-nums text-[var(--color-text-muted)] w-7 text-center">{previewScale}%</span>
                  <button
                    onClick={() => setPreviewScale((s) => Math.min(200, s + 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                    title="Zoom in"
                  >
                    +
                  </button>
                </div>
              )}
              <div className="flex gap-0.5">
                {(['preview', 'edit'] as const).map((mode) => (
                  <button
                    key={mode}
                    onClick={() => setPreviewMode(mode)}
                    className={`px-2 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                      previewMode === mode
                        ? 'bg-[var(--color-accent)] text-white'
                        : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)]'
                    }`}
                  >
                    {mode === 'preview' ? 'Preview' : 'Edit'}
                  </button>
                ))}
              </div>
            </div>
          </div>

          {previewMode === 'preview' ? (
            <div className="flex-1 overflow-auto p-4">
              <div className="markdown-content" style={{ fontSize: `${cssScale}%` }}>
                <Markdown remarkPlugins={[remarkGfm]}>
                  {stripFrontmatter(agentContent) || '*No content yet*'}
                </Markdown>
              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-hidden">
              <CodeEditor
                code={agentContent}
                filePath={agentMdPath}
                onSave={async (content) => {
                  try { await daemonCliPost('fs/write-file', { path: agentMdPath, content }) } catch (err) { console.error('[agent-editor] Save failed:', err) }
                }}
                onChange={(content) => setAgentContent(content)}
              />
            </div>
          )}
        </div>
      }
    />
  )
}
