import { useCallback, useEffect, useMemo, useState } from 'react'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'
import { AIFileEditor } from '@/components/AIFileEditor/AIFileEditor'
import { CodeEditor } from '@/components/FileViewerPane/CodeEditor'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import {
  type RoleSkill,
  roleSkillLabel,
  roleSeedSystemPrompt,
  roleSeedMessage,
} from './canonicalAgentSeeds'

/**
 * Workspace Manager / K2 Agent role-skill editor (canonical-agents PRD
 * §9.1). The SAME pattern as ProjectContextEditor — the normal AIFileEditor
 * pointed at the agent's `.k2so/agent/AGENT.md`, with the role-briefing
 * seed (organic-integration contract from canonicalAgentSeeds). The agent
 * weaves the role guidance into the existing AGENT.md; the left preview
 * shows AGENT.md updating live. The deterministic core (persist_agent_md)
 * does the backup + atomic write around the agent's merged text.
 */
export function RoleSkillEditor({
  role,
  projectPath,
  projectName,
  onClose,
}: {
  role: RoleSkill
  projectPath: string
  projectName: string
  onClose: () => void
}): React.JSX.Element {
  const [content, setContent] = useState('')
  const [previewMode, setPreviewMode] = useState<'preview' | 'edit'>('preview')

  const filePath = `${projectPath}/.k2so/agent/AGENT.md`
  // Watch the agent dir so AGENT.md edits surface live in the preview.
  const watchDir = `${projectPath}/.k2so/agent`
  const label = roleSkillLabel(role)

  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const agentCommand = useMemo(() => {
    const preset = presets.find((p) => p.id === defaultAgent) || presets.find((p) => p.enabled)
    if (!preset) return null
    return parseCommand(preset.command)
  }, [defaultAgent, presets])

  // Ensure the role SKILL.md exists before the agent runs (PRD §8.1
  // "Enable"). Idempotent + upgrade-tracked in core.
  useEffect(() => {
    daemonCliPost('skills/write-opt-in', { project_path: projectPath, skill: role }).catch((err) =>
      console.warn('[role-skill] write_opt_in_skill failed:', err),
    )
  }, [projectPath, role])

  useEffect(() => {
    daemonCliGet<{ content: string }>('fs/read-file', { path: filePath })
      .then((r) => setContent(r.content))
      .catch(() => setContent(''))
  }, [filePath])

  const handleFileChange = useCallback((c: string) => setContent(c), [])

  const terminalArgs = useMemo(() => {
    if (!agentCommand) return undefined
    const baseArgs = [...agentCommand.args]
    if (agentCommand.command === 'claude') {
      // Exact shape from the ProjectContextEditor exemplar (PRD §9.1):
      //   [...presetArgs, '--append-system-prompt', <role briefing>, <final positional message>]
      return [
        ...baseArgs,
        '--append-system-prompt',
        roleSeedSystemPrompt(role),
        roleSeedMessage(role, projectName),
      ]
    }
    return baseArgs
  }, [agentCommand, role, projectName])

  return (
    <AIFileEditor
      filePath={filePath}
      watchDir={watchDir}
      cwd={watchDir}
      command={agentCommand?.command}
      args={terminalArgs}
      title={`${label}: ${projectName}`}
      instructions={`Running the ${label} skill — the agent weaves the role guidance into AGENT.md organically, preserving your existing context.`}
      warningText="The agent integrates the role knowledge with judgment; the core backs up AGENT.md and writes the merge atomically (byte-reversible)."
      onFileChange={handleFileChange}
      onClose={onClose}
      preview={
        <div className="h-full flex flex-col">
          <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="text-xs text-[var(--color-text-muted)]">
              <span className="font-medium text-[var(--color-text-primary)]">AGENT.md</span>
              <span className="mx-2">&middot;</span>
              <span>Canonical agent source · {label}</span>
            </div>
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
          {previewMode === 'preview' ? (
            <div className="flex-1 overflow-auto p-4">
              <div className="markdown-content">
                <Markdown remarkPlugins={[remarkGfm]}>
                  {content || '*No AGENT.md yet. The assistant will weave the role guidance into it.*'}
                </Markdown>
              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-hidden">
              <CodeEditor
                code={content}
                filePath={filePath}
                onSave={async (c) => {
                  try {
                    await daemonCliPost('fs/write-file', { path: filePath, content: c })
                  } catch {
                    // best-effort manual save
                  }
                }}
                onChange={(c) => setContent(c)}
              />
            </div>
          )}
        </div>
      }
    />
  )
}
