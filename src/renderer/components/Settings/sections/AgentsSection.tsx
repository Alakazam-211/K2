import React from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useSettingsStore } from '@/stores/settings'
import {
  usePresetsStore,
  cloneDefaultInjectFlow,
  cloneDefaultInjectFlowForCommand,
  isDefaultInjectFlowForCommand,
  parseInjectFlowOrDefault,
  readPresetInjectFlowJson,
  type InjectFlowKey,
  type InjectFlowStep,
} from '@/stores/presets'
import { matchAgentPreset } from '@/lib/agent-resolve'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { KeyCombo } from '@/components/KeySymbol'
import { SettingDropdown } from '../controls/SettingControls'
import { ClaudeAuthRefreshRow } from '../shared/ClaudeAuthRefreshRow'
import type { SettingEntry } from '../searchManifest'

export const AGENTS_MANIFEST: SettingEntry[] = [
  {
    id: 'agents.default-agent',
    section: 'agents',
    group: 'Defaults',
    label: 'Default AI Agent',
    description: 'Launched with ⇧⌘T or from the assistant',
    keywords: ['agent', 'default', 'claude', 'codex', 'gemini'],
  },
  {
    id: 'agents.agent-presets',
    section: 'agents',
    label: 'Agent Presets',
    description: 'AI coding agent command palette',
    keywords: ['presets', 'commands', 'agents'],
  },
  {
    id: 'agents.reset-built-ins',
    section: 'agents',
    label: 'Reset Built-ins',
    description: 'Restore the default agent presets',
    keywords: ['reset', 'defaults', 'built-in'],
  },
  {
    id: 'agents.add-preset',
    section: 'agents',
    label: 'Add Custom Preset',
    description: 'Register your own AI agent command',
    keywords: ['preset', 'custom', 'add', 'cli'],
  },
  {
    id: 'agents.submit-keys',
    section: 'agents',
    label: 'Submit keys',
    description: 'How K2 delivers a message into this LLM’s terminal',
    keywords: ['inject', 'paste', 'keystroke', 'submit', 'esc', 'return'],
  },
  {
    id: 'agents.credentials',
    section: 'agents',
    group: 'Credentials',
    label: 'Auto-refresh credentials',
    description: 'Keep agent CLI sessions alive (Claude live; others coming soon)',
    keywords: ['credentials', 'auth', 'token', 'refresh', 'claude', 'login', 'session'],
  },
  {
    id: 'agents.cli-claude',
    section: 'agents',
    group: 'CLI Tools',
    label: 'Claude Code',
    description: 'Install instructions for the Claude Code CLI',
    keywords: ['claude', 'install', 'cli'],
  },
  {
    id: 'agents.cli-codex',
    section: 'agents',
    group: 'CLI Tools',
    label: 'OpenAI Codex',
    description: 'Install instructions for the Codex CLI',
    keywords: ['codex', 'openai', 'install', 'cli'],
  },
  {
    id: 'agents.cli-grok',
    section: 'agents',
    group: 'CLI Tools',
    label: 'Grok',
    description: 'Install instructions for the xAI Grok CLI',
    keywords: ['grok', 'xai', 'install', 'cli'],
  },
  {
    id: 'agents.cli-gemini',
    section: 'agents',
    group: 'CLI Tools',
    label: 'Gemini CLI',
    description: 'Install instructions for the Gemini CLI',
    keywords: ['gemini', 'google', 'install', 'cli'],
  },
  {
    id: 'agents.cli-cursor-agent',
    section: 'agents',
    group: 'CLI Tools',
    label: 'Cursor Agent',
    description: 'Install instructions for Cursor Agent',
    keywords: ['cursor', 'install', 'cli'],
  },
  {
    id: 'agents.cli-pi',
    section: 'agents',
    group: 'CLI Tools',
    label: 'Pi',
    description: 'Install instructions for Pi',
    keywords: ['pi', 'install', 'cli'],
  },
  {
    id: 'agents.cli-hermes',
    section: 'agents',
    group: 'CLI Tools',
    label: 'Hermes',
    description: 'Install instructions for the Nous Research Hermes Agent CLI',
    keywords: ['hermes', 'nous', 'install', 'cli'],
  },
  {
    id: 'agents.cli-opencode',
    section: 'agents',
    group: 'CLI Tools',
    label: 'OpenCode',
    description: 'Install instructions for OpenCode',
    keywords: ['opencode', 'install', 'cli'],
  },
  {
    id: 'agents.cli-goose',
    section: 'agents',
    group: 'CLI Tools',
    label: 'Goose',
    description: 'Install instructions for Goose',
    keywords: ['goose', 'block', 'install', 'cli'],
  },
  {
    id: 'agents.cli-aider',
    section: 'agents',
    group: 'CLI Tools',
    label: 'Aider',
    description: 'Install instructions for Aider',
    keywords: ['aider', 'install', 'cli', 'pip'],
  },
  {
    id: 'agents.cli-ollama',
    section: 'agents',
    group: 'CLI Tools',
    label: 'Ollama',
    description: 'Install instructions for Ollama (local models)',
    keywords: ['ollama', 'install', 'cli', 'local llm'],
  },
  {
    id: 'agents.cli-copilot',
    section: 'agents',
    group: 'CLI Tools',
    label: 'GitHub Copilot CLI',
    description: 'Install instructions for the Copilot CLI',
    keywords: ['copilot', 'github', 'install', 'cli'],
  },
  {
    id: 'agents.cli-interpreter',
    section: 'agents',
    group: 'CLI Tools',
    label: 'Interpreter',
    description: 'Install instructions for Open Interpreter',
    keywords: ['interpreter', 'open interpreter', 'install', 'cli'],
  },
]

interface PresetFormState {
  visible: boolean
  editingId: string | null
  label: string
  command: string
  icon: string
  injectFlow: InjectFlowStep[]
  injectFlowTouched: boolean
}

const INJECT_KEY_OPTIONS: { value: InjectFlowKey; label: string }[] = [
  { value: 'paste', label: 'Paste message' },
  { value: 'return', label: 'Return' },
  { value: 'esc', label: 'Esc' },
  { value: 'space', label: 'Space' },
]

const EMPTY_PRESET_FORM: PresetFormState = {
  visible: false,
  editingId: null,
  label: '',
  command: '',
  icon: '',
  injectFlow: cloneDefaultInjectFlow(),
  injectFlowTouched: false,
}

function SubmitKeysEditor({
  steps,
  command,
  onChange,
}: {
  steps: InjectFlowStep[]
  command: string
  onChange: (next: InjectFlowStep[]) => void
}): React.JSX.Element {
  const setStep = (i: number, patch: Partial<InjectFlowStep>): void => {
    onChange(steps.map((s, j) => (j === i ? { ...s, ...patch } : s)))
  }
  const remove = (i: number): void => {
    if (steps.length <= 1) return
    onChange(steps.filter((_, j) => j !== i))
  }
  const move = (i: number, dir: -1 | 1): void => {
    const j = i + dir
    if (j < 0 || j >= steps.length) return
    const next = [...steps]
    const a = next[i]
    const b = next[j]
    if (a === undefined || b === undefined) return
    next[i] = b
    next[j] = a
    onChange(next)
  }
  const add = (): void => {
    if (steps.length >= 16) return
    const hasPaste = steps.some((s) => s.key === 'paste')
    onChange([...steps, { key: hasPaste ? 'return' : 'paste', waitMs: 150 }])
  }

  return (
    <div className="space-y-2" data-settings-id="agents.submit-keys">
      <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
        Submit keys
      </div>
      <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
        How K2 delivers a message into this LLM’s terminal.{' '}
        <span className="text-[var(--color-text-secondary)] font-medium">Paste message</span> writes
        the text (not Cmd+V). Return submits. Esc / Space are extra TUI keys (e.g. Grok). Wait is
        milliseconds <span className="font-medium">after</span> that step.
      </p>
      <div className="space-y-1">
        {steps.map((step, i) => (
          <div key={i} className="flex items-center gap-1.5">
            <span
              className="w-4 shrink-0 text-[10px] text-[var(--color-text-muted)] font-mono text-right tabular-nums"
              aria-hidden="true"
            >
              {i + 1}
            </span>
            <SettingDropdown
              value={step.key}
              options={INJECT_KEY_OPTIONS}
              onChange={(key) => setStep(i, { key: key as InjectFlowKey })}
              menuAlign="left"
              className="min-w-[10rem]"
            />
            <input
              type="number"
              min={0}
              max={10000}
              data-inject-wait=""
              value={step.waitMs}
              onChange={(e) => {
                const n = Number.parseInt(e.target.value, 10)
                setStep(i, { waitMs: Number.isFinite(n) ? Math.max(0, Math.min(10000, n)) : 0 })
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') e.stopPropagation()
              }}
              className="w-16 px-1.5 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag font-mono"
            />
            <span className="text-[10px] text-[var(--color-text-muted)] font-mono">ms</span>
            <button
              type="button"
              onClick={() => move(i, -1)}
              disabled={i === 0}
              className="px-1 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] disabled:opacity-30 no-drag cursor-pointer font-mono"
              aria-label="Move step up"
            >
              ↑
            </button>
            <button
              type="button"
              onClick={() => move(i, 1)}
              disabled={i === steps.length - 1}
              className="px-1 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] disabled:opacity-30 no-drag cursor-pointer font-mono"
              aria-label="Move step down"
            >
              ↓
            </button>
            <button
              type="button"
              onClick={() => remove(i)}
              disabled={steps.length <= 1}
              className="px-1 py-0.5 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] border border-[var(--color-border)] disabled:opacity-30 no-drag cursor-pointer font-mono"
              aria-label="Remove step"
            >
              ×
            </button>
          </div>
        ))}
      </div>
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={add}
          disabled={steps.length >= 16}
          className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] disabled:opacity-30 no-drag cursor-pointer font-mono"
        >
          Add step
        </button>
        <button
          type="button"
          onClick={() => onChange(cloneDefaultInjectFlowForCommand(command))}
          className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer font-mono"
        >
          Reset to default
        </button>
      </div>
    </div>
  )
}

/** The “big 7” agent CLIs for credential auto-refresh — Claude is live. */
const CREDENTIAL_PROVIDERS: Array<{
  id: string
  label: string
  agentIcon: string
  live: boolean
}> = [
  { id: 'claude', label: 'Claude', agentIcon: 'Claude', live: true },
  { id: 'codex', label: 'Codex', agentIcon: 'Codex', live: false },
  { id: 'grok', label: 'Grok', agentIcon: 'Grok', live: false },
  { id: 'gemini', label: 'Gemini', agentIcon: 'Gemini', live: false },
  { id: 'cursor', label: 'Cursor Agent', agentIcon: 'Cursor Agent', live: false },
  { id: 'hermes', label: 'Hermes', agentIcon: 'Hermes', live: false },
  { id: 'pi', label: 'Pi', agentIcon: 'Pi', live: false },
]

function DefaultAgentPickerInline({
  presets,
}: {
  presets: { id: string; label: string; command: string }[]
}): React.JSX.Element {
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const setDefaultAgent = useSettingsStore((s) => s.setDefaultAgent)

  const agentOptions = presets.map((p) => ({
    value: p.id,
    label: p.label,
  }))
  const selectedId = matchAgentPreset(presets, defaultAgent)?.id ?? defaultAgent

  return (
    <div className="flex items-center justify-between px-3 py-2.5" data-settings-id="agents.default-agent">
      <div>
        <div className="text-xs text-[var(--color-text-secondary)]">Default AI Agent</div>
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          Launched with <KeyCombo combo="⇧⌘" />T or from the assistant
        </div>
      </div>
      <SettingDropdown value={selectedId} options={agentOptions} onChange={setDefaultAgent} />
    </div>
  )
}

function AgentCredentialsColumn(): React.JSX.Element {
  return (
    <div className="w-full" data-settings-id="agents.credentials">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Credentials</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4 leading-relaxed">
        Auto-refresh keeps agent CLI sessions alive so long runs don’t die mid-task. Claude is
        available now; the rest of the big seven are next.
      </p>
      <div className="border border-[var(--color-border)]">
        {CREDENTIAL_PROVIDERS.map((p, i) => {
          const isLast = i === CREDENTIAL_PROVIDERS.length - 1
          return (
            <div
              key={p.id}
              className={`flex items-center justify-between gap-3 px-3 py-2.5 ${
                isLast ? '' : 'border-b border-[var(--color-border)]'
              }`}
            >
              <div className="flex items-center gap-2 min-w-0">
                <AgentIcon agent={p.agentIcon} size={14} />
                <div className="min-w-0">
                  <div className="text-xs text-[var(--color-text-secondary)] truncate">{p.label}</div>
                  <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
                    Auto-refresh credentials
                  </div>
                </div>
              </div>
              {p.live ? (
                <ClaudeAuthRefreshRow embedded />
              ) : (
                <span
                  className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 flex-shrink-0 border border-[var(--color-border)] text-[var(--color-text-muted)]"
                  title="Credential auto-refresh for this agent is not available yet"
                >
                  Coming soon
                </span>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}

export function AgentsSection(): React.JSX.Element {
  const {
    presets,
    fetchPresets,
    createPreset,
    updatePreset,
    deletePreset,
    reorderPresets,
    resetPresetsToBuiltIns,
  } = usePresetsStore()
  const [presetForm, setPresetForm] = useState<PresetFormState>({ ...EMPTY_PRESET_FORM })
  const [dragIdx, setDragIdx] = useState<number | null>(null)
  const [dragOverIdx, setDragOverIdx] = useState<number | null>(null)
  const presetDragFromRef = useRef<number | null>(null)
  const presetDropRef = useRef<number | null>(null)
  const formLabelRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    void fetchPresets()
  }, [fetchPresets])

  useEffect(() => {
    if (presetForm.visible) {
      requestAnimationFrame(() => formLabelRef.current?.focus())
    }
  }, [presetForm.visible])

  const handleTogglePreset = useCallback(
    async (id: string, currentEnabled: number) => {
      await updatePreset({ id, enabled: currentEnabled ? 0 : 1 })
    },
    [updatePreset],
  )

  const handleEditPreset = useCallback((preset: (typeof presets)[number]) => {
    setPresetForm({
      visible: true,
      editingId: preset.id,
      label: preset.label,
      command: preset.command,
      icon: preset.icon ?? '',
      injectFlow: parseInjectFlowOrDefault(readPresetInjectFlowJson(preset), preset.command),
      injectFlowTouched: false,
    })
  }, [])

  const handleDeletePreset = useCallback(
    async (id: string) => {
      try {
        await deletePreset(id)
      } catch (err) {
        console.error('Failed to delete preset:', err)
      }
    },
    [deletePreset],
  )

  const openAddForm = useCallback(() => {
    setPresetForm({ ...EMPTY_PRESET_FORM })
    requestAnimationFrame(() => {
      setPresetForm({ ...EMPTY_PRESET_FORM, visible: true, injectFlow: cloneDefaultInjectFlow() })
    })
  }, [])

  const cancelForm = useCallback(() => {
    setPresetForm({ ...EMPTY_PRESET_FORM })
  }, [])

  const submitForm = useCallback(async () => {
    if (!presetForm.label.trim() || !presetForm.command.trim()) return
    try {
      const injectPayload = presetForm.injectFlowTouched
        ? isDefaultInjectFlowForCommand(presetForm.injectFlow, presetForm.command)
          ? ''
          : JSON.stringify(presetForm.injectFlow)
        : undefined
      if (presetForm.editingId) {
        await updatePreset({
          id: presetForm.editingId,
          label: presetForm.label.trim(),
          command: presetForm.command.trim(),
          icon: presetForm.icon.trim() || '',
          ...(injectPayload !== undefined ? { injectFlow: injectPayload } : {}),
        })
      } else {
        const created = await createPreset({
          label: presetForm.label.trim(),
          command: presetForm.command.trim(),
          icon: presetForm.icon.trim() || undefined,
        })
        if (injectPayload) {
          await updatePreset({ id: created.id, injectFlow: injectPayload })
        }
      }
      cancelForm()
    } catch (err) {
      console.error('Failed to save preset:', err)
    }
  }, [presetForm, cancelForm, createPreset, updatePreset])

  const handleFormKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Enter') {
        if ((e.target as HTMLElement).dataset.injectWait !== undefined) return
        e.preventDefault()
        void submitForm()
      } else if (e.key === 'Escape') {
        e.preventDefault()
        cancelForm()
      }
    },
    [submitForm, cancelForm],
  )

  const handleResetBuiltIns = useCallback(async () => {
    await resetPresetsToBuiltIns()
  }, [resetPresetsToBuiltIns])

  const handlePresetReorderMouseDown = useCallback(
    (e: React.MouseEvent, idx: number) => {
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
    },
    [reorderPresets],
  )

  return (
    <div className="flex h-full min-h-0">
      {/* Left: defaults, presets, CLI install */}
      <div className="flex-1 min-w-0 overflow-y-auto p-6 pr-3 [scrollbar-gutter:stable] space-y-8">
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Defaults</h2>
          <div className="border border-[var(--color-border)]">
            <DefaultAgentPickerInline presets={presets} />
          </div>
        </div>

        <div data-settings-id="agents.agent-presets">
          <div className="flex items-center justify-between mb-3">
            <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Agent Presets</h2>
            <button
              type="button"
              onClick={() => void handleResetBuiltIns()}
              className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer font-mono"
              data-settings-id="agents.reset-built-ins"
            >
              Reset Built-ins
            </button>
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
                {dragOverIdx === i && (
                  <div className="absolute left-0 right-0 top-0 h-[2px] bg-[var(--color-accent)] z-10" />
                )}
                {dragOverIdx === presets.length && i === presets.length - 1 && (
                  <div className="absolute left-0 right-0 bottom-0 h-[2px] bg-[var(--color-accent)] z-10" />
                )}

                <span className="w-5 flex items-center justify-center flex-shrink-0">
                  {preset.icon ? (
                    <span className="text-sm leading-none">{preset.icon}</span>
                  ) : (
                    <AgentIcon agent={preset.label} size={16} />
                  )}
                </span>

                <span className="text-xs text-[var(--color-text-primary)] font-mono w-28 truncate flex-shrink-0">
                  {preset.label}
                </span>

                <span className="text-[10px] text-[var(--color-text-muted)] font-mono flex-1 truncate">
                  {preset.command}
                </span>

                {preset.isBuiltIn ? (
                  <span className="text-[9px] text-[var(--color-text-muted)] border border-[var(--color-border)] px-1 py-0.5 flex-shrink-0 font-mono">
                    built-in
                  </span>
                ) : null}

                <button
                  type="button"
                  onClick={(e) => {
                    e.stopPropagation()
                    handleEditPreset(preset)
                  }}
                  className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] opacity-0 group-hover:opacity-100 transition-opacity no-drag cursor-pointer font-mono flex-shrink-0"
                >
                  edit
                </button>

                {!preset.isBuiltIn && (
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation()
                      void handleDeletePreset(preset.id)
                    }}
                    className="text-[10px] text-[color-mix(in_srgb,var(--color-status-error-soft)_60%,transparent)] hover:text-[var(--color-status-error-soft)] opacity-0 group-hover:opacity-100 transition-opacity no-drag cursor-pointer font-mono flex-shrink-0"
                  >
                    del
                  </button>
                )}

                <button
                  type="button"
                  onClick={(e) => {
                    e.stopPropagation()
                    void handleTogglePreset(preset.id, preset.enabled)
                  }}
                  className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
                    preset.enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
                  }`}
                >
                  <span
                    className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
                      preset.enabled ? 'translate-x-3.5' : 'translate-x-0.5'
                    }`}
                  />
                </button>
              </div>
            ))}
          </div>

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
              <SubmitKeysEditor
                steps={presetForm.injectFlow}
                command={presetForm.command}
                onChange={(injectFlow) =>
                  setPresetForm((s) => ({ ...s, injectFlow, injectFlowTouched: true }))
                }
              />
              <div className="flex items-center gap-2 justify-end">
                <button
                  type="button"
                  onClick={cancelForm}
                  className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer font-mono"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={() => void submitForm()}
                  className="px-3 py-1 text-xs bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:bg-[var(--color-accent)]/80 no-drag cursor-pointer font-mono"
                >
                  {presetForm.editingId ? 'Save' : 'Add'}
                </button>
              </div>
            </div>
          )}

          <div className="mt-2">
            <button
              type="button"
              onClick={openAddForm}
              className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer font-mono"
              data-settings-id="agents.add-preset"
            >
              + Add Custom Preset
            </button>
          </div>
        </div>

        <CLIInstallGuide />
      </div>

      {/* Right: credential auto-refresh for the big 7 */}
      <div className="flex-1 min-w-0 overflow-y-auto border-l border-[var(--color-border)] p-6 pl-6 pr-3 [scrollbar-gutter:stable]">
        <AgentCredentialsColumn />
      </div>
    </div>
  )
}

// ── CLI Install Guide ───────────────────────────────────────────────
// Order matches built-in Agent Presets (db seed / Reset Built-ins).
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
    notes:
      'Requires Node.js 18+. After install, run "claude" to authenticate with your Anthropic account.',
  },
  {
    name: 'OpenAI Codex',
    command: 'codex',
    installCommand: 'npm install -g @openai/codex',
    docs: 'https://github.com/openai/codex',
    notes:
      'Requires Node.js 22+. After install, set your OPENAI_API_KEY or log in via "codex --login".',
  },
  {
    name: 'Grok',
    command: 'grok',
    installCommand: 'curl -fsSL https://x.ai/cli/install.sh | bash',
    docs: 'https://docs.x.ai/build',
    notes:
      "xAI's terminal coding agent. On first launch it opens a browser to sign in; for headless use set the XAI_API_KEY environment variable. Skip approval prompts (\"yolo\" mode) with \"grok --always-approve\".",
  },
  {
    name: 'Gemini CLI',
    command: 'gemini',
    installCommand: 'npm install -g @anthropic-ai/gemini-cli',
    docs: 'https://geminicli.com',
    notes: 'Requires Node.js 18+. Authenticate with your Google account on first run.',
  },
  {
    name: 'Cursor Agent',
    command: 'cursor-agent',
    installCommand: 'npm install -g cursor-agent',
    docs: 'https://docs.cursor.com',
    notes: 'The standalone CLI agent from Cursor. Requires a Cursor subscription.',
  },
  {
    name: 'Pi',
    command: 'pi',
    installCommand: 'npm install -g @mariozechner/pi-coding-agent',
    docs: 'https://github.com/badlogic/pi-mono',
    notes:
      'Minimal coding agent with 15+ LLM providers. Supports OAuth login (/login) for Claude, Copilot, Gemini subscriptions, or use API keys directly.',
  },
  {
    name: 'Hermes',
    command: 'hermes',
    installCommand: 'curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash',
    docs: 'https://hermes-agent.nousresearch.com/docs/getting-started/quickstart',
    notes:
      'Nous Research Hermes Agent. After install, run "hermes" and complete provider setup. Desktop installers are also available from the Hermes site.',
  },
  {
    name: 'OpenCode',
    command: 'opencode',
    installCommand: 'curl -fsSL https://opencode.ai/install | bash',
    docs: 'https://opencode.ai',
    notes: 'A terminal-based AI coding assistant. Supports multiple model providers.',
  },
  {
    name: 'Goose',
    command: 'goose',
    installCommand:
      'curl -fsSL https://github.com/block/goose/releases/latest/download/install.sh | bash',
    docs: 'https://github.com/block/goose',
    notes: 'An open-source AI developer agent from Block. Supports multiple model providers.',
  },
  {
    name: 'Aider',
    command: 'aider',
    installCommand: 'pip install aider-chat',
    docs: 'https://aider.chat/docs/install.html',
    notes:
      'Requires Python 3.9+. Configure your API key for the model provider you want to use.',
  },
  {
    name: 'Ollama',
    command: 'ollama',
    installCommand: 'curl -fsSL https://ollama.ai/install.sh | sh',
    docs: 'https://ollama.ai',
    notes: 'Run large language models locally. After install, pull a model with "ollama pull llama3".',
  },
  {
    name: 'GitHub Copilot CLI',
    command: 'copilot',
    installCommand: 'npm install -g @anthropic-ai/copilot-cli',
    docs: 'https://docs.github.com/en/copilot/how-tos/copilot-cli',
    notes:
      'Requires an active GitHub Copilot subscription. Authenticate with "gh auth login" first.',
  },
  {
    name: 'Interpreter',
    command: 'interpreter',
    installCommand: 'pip install open-interpreter',
    docs: 'https://docs.openinterpreter.com',
    notes: 'Open Interpreter — natural language interface to your computer. Configure an LLM provider after install.',
  },
]

function CLIInstallGuide(): React.JSX.Element {
  const [expandedIdx, setExpandedIdx] = useState<number | null>(null)
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null)

  const handleCopy = useCallback((installCommand: string, idx: number) => {
    void navigator.clipboard.writeText(installCommand)
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
          const settingsId = `agents.cli-${entry.command === 'cursor-agent' ? 'cursor-agent' : entry.command}`

          return (
            <div
              key={entry.command}
              className={i < CLI_INSTALL_ENTRIES.length - 1 ? 'border-b border-[var(--color-border)]' : ''}
              data-settings-id={settingsId}
            >
              <button
                type="button"
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

              {isExpanded && (
                <div className="px-3 pb-3 pt-0 ml-5 space-y-2">
                  <div className="flex items-center gap-2">
                    <code className="flex-1 text-[11px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)] px-2 py-1.5 text-[var(--color-text-primary)] select-all">
                      {entry.installCommand}
                    </code>
                    <button
                      type="button"
                      onClick={() => handleCopy(entry.installCommand, i)}
                      className="flex-shrink-0 px-2 py-1.5 text-[10px] font-mono border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
                      style={{
                        color: isCopied ? 'var(--color-status-ok)' : 'var(--color-text-muted)',
                      }}
                    >
                      {isCopied ? 'Copied!' : 'Copy'}
                    </button>
                  </div>

                  {entry.notes && (
                    <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
                      {entry.notes}
                    </p>
                  )}

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
