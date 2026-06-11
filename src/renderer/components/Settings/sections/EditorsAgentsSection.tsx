import React from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore } from '@/stores/presets'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { KeyCombo } from '@/components/KeySymbol'
import { SettingDropdown } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'

export const EDITORS_AGENTS_MANIFEST: SettingEntry[] = [
  // Defaults
  { id: 'editors-agents.default-editor', section: 'editors-agents', group: 'Defaults', label: 'Default Editor', description: 'Opens files and projects with this editor', keywords: ['editor', 'default', 'cursor', 'vscode', 'zed'] },
  { id: 'editors-agents.default-terminal', section: 'editors-agents', group: 'Defaults', label: 'Default Terminal', description: 'Right-click a tab to open in this terminal', keywords: ['terminal', 'default', 'iterm', 'warp', 'ghostty'] },
  { id: 'editors-agents.default-agent', section: 'editors-agents', group: 'Defaults', label: 'Default AI Agent', description: 'Launched with ⇧⌘T or from the assistant', keywords: ['agent', 'default', 'claude', 'codex', 'gemini'] },
  // Detected apps
  { id: 'editors-agents.detected-editors', section: 'editors-agents', label: 'Detected Editors', description: 'Editors discovered on your system', keywords: ['detected', 'editors', 'scan', 'refresh'] },
  { id: 'editors-agents.terminal-apps', section: 'editors-agents', label: 'Terminal Apps', description: 'Terminal emulators detected on your system', keywords: ['terminal', 'apps', 'detected'] },
  // Presets
  { id: 'editors-agents.agent-presets', section: 'editors-agents', label: 'Agent Presets', description: 'AI coding agent command palette', keywords: ['presets', 'commands', 'agents'] },
  { id: 'editors-agents.reset-built-ins', section: 'editors-agents', label: 'Reset Built-ins', description: 'Restore the default agent presets', keywords: ['reset', 'defaults', 'built-in'] },
  { id: 'editors-agents.add-preset', section: 'editors-agents', label: 'Add Custom Preset', description: 'Register your own AI agent command', keywords: ['preset', 'custom', 'add', 'cli'] },
  // CLI install guide — one entry per shipped CLI tool
  { id: 'editors-agents.cli-claude', section: 'editors-agents', group: 'CLI Tools', label: 'Claude Code', description: 'Install instructions for the Claude Code CLI', keywords: ['claude', 'install', 'cli'] },
  { id: 'editors-agents.cli-codex', section: 'editors-agents', group: 'CLI Tools', label: 'OpenAI Codex', description: 'Install instructions for the Codex CLI', keywords: ['codex', 'openai', 'install', 'cli'] },
  { id: 'editors-agents.cli-gemini', section: 'editors-agents', group: 'CLI Tools', label: 'Gemini CLI', description: 'Install instructions for the Gemini CLI', keywords: ['gemini', 'google', 'install', 'cli'] },
  { id: 'editors-agents.cli-copilot', section: 'editors-agents', group: 'CLI Tools', label: 'GitHub Copilot CLI', description: 'Install instructions for the Copilot CLI', keywords: ['copilot', 'github', 'install', 'cli'] },
  { id: 'editors-agents.cli-aider', section: 'editors-agents', group: 'CLI Tools', label: 'Aider', description: 'Install instructions for Aider', keywords: ['aider', 'install', 'cli', 'pip'] },
  { id: 'editors-agents.cli-cursor-agent', section: 'editors-agents', group: 'CLI Tools', label: 'Cursor Agent', description: 'Install instructions for Cursor Agent', keywords: ['cursor', 'install', 'cli'] },
  { id: 'editors-agents.cli-opencode', section: 'editors-agents', group: 'CLI Tools', label: 'OpenCode', description: 'Install instructions for OpenCode', keywords: ['opencode', 'install', 'cli'] },
  { id: 'editors-agents.cli-goose', section: 'editors-agents', group: 'CLI Tools', label: 'Goose', description: 'Install instructions for Goose', keywords: ['goose', 'block', 'install', 'cli'] },
  { id: 'editors-agents.cli-pi', section: 'editors-agents', group: 'CLI Tools', label: 'Pi', description: 'Install instructions for Pi', keywords: ['pi', 'install', 'cli'] },
  { id: 'editors-agents.cli-ollama', section: 'editors-agents', group: 'CLI Tools', label: 'Ollama', description: 'Install instructions for Ollama (local models)', keywords: ['ollama', 'install', 'cli', 'local llm'] },
]

interface EditorDetected {
  id: string
  label: string
  macApp: string
  cliCommand: string
  installed: boolean
  type: 'editor' | 'terminal'
}

interface PresetFormState {
  visible: boolean
  editingId: string | null
  label: string
  command: string
  icon: string
}

function DefaultAgentPickerInline({ presets }: { presets: { id: string; label: string; command: string }[] }): React.JSX.Element {
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const setDefaultAgent = useSettingsStore((s) => s.setDefaultAgent)

  const agentOptions = presets.map((p) => ({
    value: p.command.split(/\s+/)[0],
    label: p.label,
  }))

  return (
    <div className="flex items-center justify-between px-3 py-2.5">
      <div>
        <div className="text-xs text-[var(--color-text-secondary)]">Default AI Agent</div>
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Launched with <KeyCombo combo="⇧⌘" />T or from the assistant</div>
      </div>
      <SettingDropdown
        value={defaultAgent}
        options={agentOptions}
        onChange={setDefaultAgent}
      />
    </div>
  )
}

export function EditorsAgentsSection(): React.JSX.Element {
  const {
    presets,
    fetchPresets,
    createPreset,
    updatePreset,
    deletePreset,
    reorderPresets,
    resetPresetsToBuiltIns,
  } = usePresetsStore()
  const projectSettings = useSettingsStore((s) => s.projectSettings)
  const updateProjectSetting = useSettingsStore((s) => s.updateProjectSetting)
  const [editors, setEditors] = useState<EditorDetected[]>([])
  const [editorsLoading, setEditorsLoading] = useState(false)
  const [presetForm, setPresetForm] = useState<PresetFormState>({
    visible: false,
    editingId: null,
    label: '',
    command: '',
    icon: ''
  })
  const [dragIdx, setDragIdx] = useState<number | null>(null)
  const [dragOverIdx, setDragOverIdx] = useState<number | null>(null)
  const presetDragFromRef = useRef<number | null>(null)
  const presetDropRef = useRef<number | null>(null)
  const formLabelRef = useRef<HTMLInputElement>(null)

  const loadEditors = useCallback(async () => {
    setEditorsLoading(true)
    try {
      const result = await invoke<any[]>('projects_get_all_editors')
      setEditors(result)
    } catch (err) {
      console.error('Failed to load editors:', err)
    } finally {
      setEditorsLoading(false)
    }
  }, [])

  const refreshEditors = useCallback(async () => {
    setEditorsLoading(true)
    try {
      const result = await invoke<any[]>('projects_refresh_editors')
      setEditors(result)
    } catch (err) {
      console.error('Failed to refresh editors:', err)
    } finally {
      setEditorsLoading(false)
    }
  }, [])

  useEffect(() => {
    loadEditors()
    fetchPresets()
  }, [loadEditors, fetchPresets])

  useEffect(() => {
    if (presetForm.visible) {
      requestAnimationFrame(() => formLabelRef.current?.focus())
    }
  }, [presetForm.visible])

  const handleTogglePreset = useCallback(async (id: string, currentEnabled: number) => {
    await updatePreset({ id, enabled: currentEnabled ? 0 : 1 })
  }, [updatePreset])

  const handleEditPreset = useCallback((preset: typeof presets[number]) => {
    setPresetForm({
      visible: true,
      editingId: preset.id,
      label: preset.label,
      command: preset.command,
      icon: preset.icon ?? ''
    })
  }, [])

  const handleDeletePreset = useCallback(async (id: string) => {
    try {
      await deletePreset(id)
    } catch (err) {
      console.error('Failed to delete preset:', err)
    }
  }, [deletePreset])

  const openAddForm = useCallback(() => {
    setPresetForm({ visible: false, editingId: null, label: '', command: '', icon: '' })
    requestAnimationFrame(() => {
      setPresetForm({ visible: true, editingId: null, label: '', command: '', icon: '' })
    })
  }, [])

  const cancelForm = useCallback(() => {
    setPresetForm({ visible: false, editingId: null, label: '', command: '', icon: '' })
  }, [])

  const submitForm = useCallback(async () => {
    if (!presetForm.label.trim() || !presetForm.command.trim()) return
    try {
      if (presetForm.editingId) {
        await updatePreset({
          id: presetForm.editingId,
          label: presetForm.label.trim(),
          command: presetForm.command.trim(),
          icon: presetForm.icon.trim() || ''
        })
      } else {
        await createPreset({
          label: presetForm.label.trim(),
          command: presetForm.command.trim(),
          icon: presetForm.icon.trim() || undefined
        })
      }
      cancelForm()
    } catch (err) {
      console.error('Failed to save preset:', err)
    }
  }, [presetForm, cancelForm, createPreset, updatePreset])

  const handleFormKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault()
      submitForm()
    } else if (e.key === 'Escape') {
      e.preventDefault()
      cancelForm()
    }
  }, [submitForm, cancelForm])

  const handleResetBuiltIns = useCallback(async () => {
    await resetPresetsToBuiltIns()
  }, [resetPresetsToBuiltIns])

  const handlePresetReorderMouseDown = useCallback((e: React.MouseEvent, idx: number) => {
    if (e.button !== 0) return
    const startY = e.clientY
    let started = false

    const handleMouseMove = (ev: MouseEvent): void => {
      if (!started && Math.abs(ev.clientY - startY) > 5) {
        started = true
        presetDragFromRef.current = idx
        setDragIdx(idx)
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }
      if (!started) return

      const container = document.querySelector('[data-preset-reorder-container]')
      if (!container) return
      const items = container.querySelectorAll('[data-preset-reorder-index]')
      let dropIdx = 0
      for (let i = 0; i < items.length; i++) {
        const rect = items[i].getBoundingClientRect()
        if (ev.clientY > rect.top + rect.height / 2) dropIdx = i + 1
      }
      presetDropRef.current = dropIdx
      setDragOverIdx(dropIdx)
    }

    const handleMouseUp = async (): Promise<void> => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''

      if (started) {
        const fromIdx = presetDragFromRef.current
        const dropIdx = presetDropRef.current
        if (fromIdx !== null && dropIdx !== null && fromIdx !== dropIdx && fromIdx !== dropIdx - 1) {
          const currentPresets = usePresetsStore.getState().presets
          const sorted = [...currentPresets]
          const [moved] = sorted.splice(fromIdx, 1)
          const insertAt = dropIdx > fromIdx ? dropIdx - 1 : dropIdx
          sorted.splice(insertAt, 0, moved)
          await reorderPresets(sorted.map((p) => p.id))
        }
      }

      setDragIdx(null)
      setDragOverIdx(null)
      presetDragFromRef.current = null
      presetDropRef.current = null
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [reorderPresets])

  const editorApps = editors.filter((e) => e.type === 'editor')
  const terminalApps = editors.filter((e) => e.type === 'terminal')

  return (
    <div className="max-w-2xl space-y-8">
      {/* ── Defaults ── */}
      <div>
        <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Defaults</h2>
        <div className="border border-[var(--color-border)]">
          <div className="flex items-center justify-between px-3 py-2.5 border-b border-[var(--color-border)]">
            <div>
              <div className="text-xs text-[var(--color-text-secondary)]">Default Editor</div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Opens files and projects with this editor</div>
            </div>
            <SettingDropdown
              value={(projectSettings['__global__'] as any)?.defaultEditor ?? editorApps.find((e) => e.installed)?.label ?? 'Cursor'}
              options={editorApps.filter((e) => e.installed).map((ed) => ({ value: ed.label, label: ed.label }))}
              onChange={(v) => updateProjectSetting('__global__', 'defaultEditor', v)}
            />
          </div>
          {/* Default Terminal */}
          <div className="flex items-center justify-between px-3 py-2.5 border-b border-[var(--color-border)]">
            <div>
              <div className="text-xs text-[var(--color-text-secondary)]">Default Terminal</div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Right-click a tab to open in this terminal</div>
            </div>
            <SettingDropdown
              value={(projectSettings['__global__'] as any)?.defaultTerminal ?? 'Terminal'}
              options={[
                { value: 'Terminal', label: 'Terminal' },
                ...terminalApps.filter((e) => e.installed).map((ed) => ({ value: ed.label, label: ed.label })),
              ]}
              onChange={(v) => updateProjectSetting('__global__', 'defaultTerminal', v)}
            />
          </div>
          <DefaultAgentPickerInline presets={presets} />
        </div>
      </div>

      {/* ── Editors ── */}
      <div>
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Detected Editors</h2>
          <button
            onClick={refreshEditors}
            disabled={editorsLoading}
            className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default font-mono"
          >
            {editorsLoading ? 'Scanning...' : 'Refresh'}
          </button>
        </div>

        <div className="border border-[var(--color-border)]">
          {editorApps.map((editor, i) => (
            <div
              key={editor.id}
              className={`flex items-center gap-3 px-3 py-1.5 ${
                i < editorApps.length - 1 ? 'border-b border-[var(--color-border)]' : ''
              }`}
            >
              <span
                className={`w-1.5 h-1.5 flex-shrink-0 ${
                  editor.installed ? 'bg-green-500' : 'bg-red-500/60'
                }`}
              />
              <span className="text-xs text-[var(--color-text-primary)] font-mono flex-1">
                {editor.label}
              </span>
              <span className="text-[10px] text-[var(--color-text-muted)] font-mono">
                {editor.installed ? editor.cliCommand || editor.macApp : 'not found'}
              </span>
            </div>
          ))}
        </div>

        {/* Terminal apps */}
        {terminalApps.length > 0 && (
          <div className="mt-3">
            <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1 px-1">
              Terminal Apps
            </div>
            <div className="border border-[var(--color-border)]">
              {terminalApps.map((app, i) => (
                <div
                  key={app.id}
                  className={`flex items-center gap-3 px-3 py-1.5 ${
                    i < terminalApps.length - 1 ? 'border-b border-[var(--color-border)]' : ''
                  }`}
                >
                  <span
                    className={`w-1.5 h-1.5 flex-shrink-0 ${
                      app.installed ? 'bg-green-500' : 'bg-red-500/60'
                    }`}
                  />
                  <span className="text-xs text-[var(--color-text-primary)] font-mono flex-1">
                    {app.label}
                  </span>
                  <span className="text-[10px] text-[var(--color-text-muted)] font-mono">
                    {app.installed ? 'installed' : 'not found'}
                  </span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* ── Agent Presets ── */}
      <div>
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Agent Presets</h2>
          <div className="flex items-center gap-2">
            <button
              onClick={handleResetBuiltIns}
              className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer font-mono"
            >
              Reset Built-ins
            </button>
          </div>
        </div>

        <div className="border border-[var(--color-border)]" data-preset-reorder-container>
          {presets.map((preset, i) => (
            <div
              key={preset.id}
              data-preset-reorder-index={i}
              onMouseDown={(e) => handlePresetReorderMouseDown(e, i)}
              className={`relative flex items-center gap-2 px-3 py-1.5 group transition-colors select-none ${
                i < presets.length - 1 ? 'border-b border-[var(--color-border)]' : ''
              } ${dragIdx === i ? 'opacity-30' : ''} cursor-grab active:cursor-grabbing`}
            >
              {dragOverIdx === i && <div className="absolute left-0 right-0 top-0 h-[2px] bg-[var(--color-accent)] z-10" />}
              {dragOverIdx === presets.length && i === presets.length - 1 && <div className="absolute left-0 right-0 bottom-0 h-[2px] bg-[var(--color-accent)] z-10" />}

              {/* Icon — emoji override, otherwise custom drawn icon */}
              <span className="w-5 flex items-center justify-center flex-shrink-0">
                {preset.icon ? (
                  <span className="text-sm leading-none">{preset.icon}</span>
                ) : (
                  <AgentIcon agent={preset.label} size={16} />
                )}
              </span>

              {/* Label */}
              <span className="text-xs text-[var(--color-text-primary)] font-mono w-28 truncate flex-shrink-0">
                {preset.label}
              </span>

              {/* Command */}
              <span className="text-[10px] text-[var(--color-text-muted)] font-mono flex-1 truncate">
                {preset.command}
              </span>

              {/* Built-in badge */}
              {preset.isBuiltIn ? (
                <span className="text-[9px] text-[var(--color-text-muted)] border border-[var(--color-border)] px-1 py-0.5 flex-shrink-0 font-mono">
                  built-in
                </span>
              ) : null}

              {/* Edit button */}
              <button
                onClick={(e) => { e.stopPropagation(); handleEditPreset(preset) }}
                className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] opacity-0 group-hover:opacity-100 transition-opacity no-drag cursor-pointer font-mono flex-shrink-0"
              >
                edit
              </button>

              {/* Delete button (custom only) */}
              {!preset.isBuiltIn && (
                <button
                  onClick={(e) => { e.stopPropagation(); handleDeletePreset(preset.id) }}
                  className="text-[10px] text-red-400/60 hover:text-red-400 opacity-0 group-hover:opacity-100 transition-opacity no-drag cursor-pointer font-mono flex-shrink-0"
                >
                  del
                </button>
              )}

              {/* Enabled toggle */}
              <button
                onClick={(e) => { e.stopPropagation(); handleTogglePreset(preset.id, preset.enabled) }}
                className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
                  preset.enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
                }`}
              >
                <span
                  className={`w-2.5 h-2.5 bg-white block transition-transform ${
                    preset.enabled ? 'translate-x-3.5' : 'translate-x-0.5'
                  }`}
                />
              </button>
            </div>
          ))}
        </div>

        {/* Inline edit/add form */}
        {presetForm.visible && (
          <div
            className="mt-2 border border-[var(--color-border)] bg-[var(--color-bg-surface)] p-3 space-y-2"
            onKeyDown={handleFormKeyDown}
          >
            <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1">
              {presetForm.editingId ? 'Edit Preset' : 'New Preset'}
            </div>
            <div className="flex items-center gap-2">
              <input
                type="text"
                value={presetForm.icon}
                onChange={(e) => setPresetForm((s) => ({ ...s, icon: e.target.value }))}
                placeholder="Icon"
                className="w-10 px-1 py-1 text-center text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)] no-drag font-mono"
              />
              <input
                ref={formLabelRef}
                type="text"
                value={presetForm.label}
                onChange={(e) => setPresetForm((s) => ({ ...s, label: e.target.value }))}
                placeholder="Label"
                className="w-28 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)] no-drag font-mono"
              />
              <input
                type="text"
                value={presetForm.command}
                onChange={(e) => setPresetForm((s) => ({ ...s, command: e.target.value }))}
                placeholder="Command (e.g. aider --model gpt-4)"
                className="flex-1 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)] no-drag font-mono"
              />
            </div>
            <div className="flex items-center gap-2 justify-end">
              <button
                onClick={cancelForm}
                className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer font-mono"
              >
                Cancel
              </button>
              <button
                onClick={submitForm}
                className="px-3 py-1 text-xs bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/80 no-drag cursor-pointer font-mono"
              >
                {presetForm.editingId ? 'Save' : 'Add'}
              </button>
            </div>
          </div>
        )}

        {/* Add custom preset button */}
        <div className="mt-2">
          <button
            onClick={openAddForm}
            className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer font-mono"
          >
            + Add Custom Preset
          </button>
        </div>
      </div>

      {/* ── CLI Install Guide ── */}
      <CLIInstallGuide />
    </div>
  )
}

// ── CLI Install Guide Data ──────────────────────────────────────────
const CLI_INSTALL_ENTRIES: {
  name: string
  command: string
  installCommand: string
  docs: string
  notes?: string
}[] = [
  {
    name: 'Claude Code',
    command: 'claude',
    installCommand: 'npm install -g @anthropic-ai/claude-code',
    docs: 'https://docs.anthropic.com/en/docs/claude-code',
    notes: 'Requires Node.js 18+. After install, run "claude" to authenticate with your Anthropic account.'
  },
  {
    name: 'OpenAI Codex',
    command: 'codex',
    installCommand: 'npm install -g @openai/codex',
    docs: 'https://github.com/openai/codex',
    notes: 'Requires Node.js 22+. After install, set your OPENAI_API_KEY or log in via "codex --login".'
  },
  {
    name: 'Gemini CLI',
    command: 'gemini',
    installCommand: 'npm install -g @anthropic-ai/gemini-cli',
    docs: 'https://geminicli.com',
    notes: 'Requires Node.js 18+. Authenticate with your Google account on first run.'
  },
  {
    name: 'GitHub Copilot CLI',
    command: 'copilot',
    installCommand: 'npm install -g @anthropic-ai/copilot-cli',
    docs: 'https://docs.github.com/en/copilot/how-tos/copilot-cli',
    notes: 'Requires an active GitHub Copilot subscription. Authenticate with "gh auth login" first.'
  },
  {
    name: 'Aider',
    command: 'aider',
    installCommand: 'pip install aider-chat',
    docs: 'https://aider.chat/docs/install.html',
    notes: 'Requires Python 3.9+. Configure your API key for the model provider you want to use.'
  },
  {
    name: 'Cursor Agent',
    command: 'cursor-agent',
    installCommand: 'npm install -g cursor-agent',
    docs: 'https://docs.cursor.com',
    notes: 'The standalone CLI agent from Cursor. Requires a Cursor subscription.'
  },
  {
    name: 'OpenCode',
    command: 'opencode',
    installCommand: 'curl -fsSL https://opencode.ai/install | bash',
    docs: 'https://opencode.ai',
    notes: 'A terminal-based AI coding assistant. Supports multiple model providers.'
  },
  {
    name: 'Goose',
    command: 'goose',
    installCommand: 'curl -fsSL https://github.com/block/goose/releases/latest/download/install.sh | bash',
    docs: 'https://github.com/block/goose',
    notes: 'An open-source AI developer agent from Block. Supports multiple model providers.'
  },
  {
    name: 'Pi',
    command: 'pi',
    installCommand: 'npm install -g @mariozechner/pi-coding-agent',
    docs: 'https://github.com/badlogic/pi-mono',
    notes: 'Minimal coding agent with 15+ LLM providers. Supports OAuth login (/login) for Claude, Copilot, Gemini subscriptions, or use API keys directly.'
  },
  {
    name: 'Ollama',
    command: 'ollama',
    installCommand: 'curl -fsSL https://ollama.ai/install.sh | sh',
    docs: 'https://ollama.ai',
    notes: 'Run large language models locally. After install, pull a model with "ollama pull llama3".'
  }
]

function CLIInstallGuide(): React.JSX.Element {
  const [expandedIdx, setExpandedIdx] = useState<number | null>(null)
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null)

  const handleCopy = useCallback((installCommand: string, idx: number) => {
    navigator.clipboard.writeText(installCommand)
    setCopiedIdx(idx)
    setTimeout(() => setCopiedIdx(null), 2000)
  }, [])

  return (
    <div>
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">CLI Tools Setup</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-3">
        Install instructions for each AI coding agent. Click to expand.
      </p>

      <div className="border border-[var(--color-border)]">
        {CLI_INSTALL_ENTRIES.map((entry, i) => {
          const isExpanded = expandedIdx === i
          const isCopied = copiedIdx === i

          return (
            <div
              key={entry.command}
              className={i < CLI_INSTALL_ENTRIES.length - 1 ? 'border-b border-[var(--color-border)]' : ''}
            >
              {/* Header row — clickable to expand */}
              <button
                className="w-full flex items-center gap-3 px-3 py-2 text-left hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer"
                onClick={() => setExpandedIdx(isExpanded ? null : i)}
              >
                <svg
                  width="8"
                  height="8"
                  viewBox="0 0 8 8"
                  fill="currentColor"
                  className={`flex-shrink-0 text-[var(--color-text-muted)] transition-transform ${isExpanded ? 'rotate-90' : ''}`}
                >
                  <polygon points="1,0 7,4 1,8" />
                </svg>
                <span className="text-xs text-[var(--color-text-primary)] font-mono flex-1">
                  {entry.name}
                </span>
                <span className="text-[10px] text-[var(--color-text-muted)] font-mono">
                  {entry.command}
                </span>
              </button>

              {/* Expanded detail */}
              {isExpanded && (
                <div className="px-3 pb-3 pt-0 ml-5 space-y-2">
                  {/* Install command with copy button */}
                  <div className="flex items-center gap-2">
                    <code className="flex-1 text-[11px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)] px-2 py-1.5 text-[var(--color-text-primary)] select-all">
                      {entry.installCommand}
                    </code>
                    <button
                      onClick={() => handleCopy(entry.installCommand, i)}
                      className="flex-shrink-0 px-2 py-1.5 text-[10px] font-mono border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
                      style={{ color: isCopied ? '#22c55e' : 'var(--color-text-muted)' }}
                    >
                      {isCopied ? 'Copied!' : 'Copy'}
                    </button>
                  </div>

                  {/* Notes */}
                  {entry.notes && (
                    <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
                      {entry.notes}
                    </p>
                  )}

                  {/* Docs link */}
                  <a
                    href={entry.docs}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="inline-block text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent)]/80 font-mono transition-colors"
                  >
                    Documentation →
                  </a>
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}
