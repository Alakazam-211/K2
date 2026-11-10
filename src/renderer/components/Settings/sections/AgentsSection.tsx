import React from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useSettingsStore } from '@/stores/settings'
import { useConnectHostStore } from '@/stores/connect-host'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import {
  usePresetsStore,
  cloneDefaultInjectFlow,
  cloneDefaultInjectFlowForCommand,
  isDefaultInjectFlowForCommand,
  parseInjectFlowOrDefault,
  readPresetInjectFlowJson,
  type AgentPreset,
  type InjectFlowKey,
  type InjectFlowStep,
} from '@/stores/presets'
import { matchAgentPreset } from '@/lib/agent-resolve'
import { daemonCliGet } from '@/lib/daemon-cli'
import {
  CLI_INSTALL_COMMANDS,
  ensureOneCli,
  installingCliLabel,
  isInstallableCli,
  useCliInstallStore,
  type InstallableCli,
} from '@/lib/ensure-cli'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { primaryScope } from '@/kessel/server-scope'
import { SettingDropdown, SettingRow, SettingsGroup, SETTING_ROW_HINT } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { metaChordLabel } from '@/lib/shortcut-label'
import type { DesktopOs } from '@/lib/desktop-chrome'
import { Button, Callout, Input, OverflowMenu, Surface, Tag, Toggle } from '@/components/ui'

/** "Opens with ⇧⌘T …" with the chord this platform really uses. */
export function defaultAgentLaunchCopy(os?: DesktopOs): string {
  const chord = metaChordLabel('Shift+Meta+T', os)
  return chord
    ? `Opens with ${chord} and when the assistant starts an agent.`
    : 'Opens from the tab + menu and when the assistant starts an agent.'
}

// ── Agent tools ─────────────────────────────────────────────────────
// Order matches the built-in presets (db seed / Restore built-ins).
interface AgentTool {
  name: string
  /** The program K2 runs, and the search id's suffix. */
  command: string
  installCommand: string
  docs: string
  /** What to do after installing, in one or two plain sentences. */
  notes: string
  keywords: string[]
}

const SUBS_NOTE = 'After installing, add your subscription in Language Models → Subs / Tokens.'

const AGENT_TOOLS: AgentTool[] = [
  { name: 'Claude Code', command: 'claude', installCommand: CLI_INSTALL_COMMANDS.claude, docs: 'https://docs.anthropic.com/en/docs/claude-code', notes: SUBS_NOTE, keywords: ['claude', 'anthropic'] },
  { name: 'OpenAI Codex', command: 'codex', installCommand: CLI_INSTALL_COMMANDS.codex, docs: 'https://github.com/openai/codex', notes: SUBS_NOTE, keywords: ['codex', 'openai'] },
  {
    name: 'Grok',
    command: 'grok',
    installCommand: CLI_INSTALL_COMMANDS.grok,
    docs: 'https://docs.x.ai/build',
    notes: `${SUBS_NOTE} Without a subscription, set the XAI_API_KEY environment variable instead.`,
    keywords: ['grok', 'xai'],
  },
  { name: 'Gemini CLI', command: 'gemini', installCommand: CLI_INSTALL_COMMANDS.gemini, docs: 'https://geminicli.com', notes: 'Needs Node.js 18 or later. Sign in with your Google account the first time it runs.', keywords: ['gemini', 'google'] },
  {
    name: 'Cursor Agent',
    command: 'cursor-agent',
    // Cursor's official installer (cursor.com/docs/cli/installation). The
    // npm package named `cursor-agent` is a third party's, not Cursor's.
    installCommand: 'curl https://cursor.com/install -fsS | bash',
    docs: 'https://cursor.com/docs/cli/installation',
    notes: 'Cursor’s agent for the terminal. Needs a Cursor subscription.',
    keywords: ['cursor'],
  },
  { name: 'Pi', command: 'pi', installCommand: 'npm install -g @mariozechner/pi-coding-agent', docs: 'https://github.com/badlogic/pi-mono', notes: 'A small coding agent that works with many model providers. Sign in with /login, or use an API key.', keywords: ['pi'] },
  { name: 'Hermes', command: 'hermes', installCommand: 'curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash', docs: 'https://hermes-agent.nousresearch.com/docs/getting-started/quickstart', notes: 'Nous Research’s agent. After installing, run hermes once to choose a model provider.', keywords: ['hermes', 'nous'] },
  { name: 'OpenCode', command: 'opencode', installCommand: 'curl -fsSL https://opencode.ai/install | bash', docs: 'https://opencode.ai', notes: 'A coding agent for the terminal that works with many model providers.', keywords: ['opencode'] },
  { name: 'Goose', command: 'goose', installCommand: 'curl -fsSL https://github.com/block/goose/releases/latest/download/install.sh | bash', docs: 'https://github.com/block/goose', notes: 'An open-source coding agent from Block that works with many model providers.', keywords: ['goose', 'block'] },
  { name: 'Aider', command: 'aider', installCommand: 'pip install aider-chat', docs: 'https://aider.chat/docs/install.html', notes: 'Needs Python 3.9 or later. Add an API key for the model provider you use.', keywords: ['aider', 'pip'] },
  { name: 'Ollama', command: 'ollama', installCommand: 'curl -fsSL https://ollama.com/install.sh | sh', docs: 'https://ollama.com', notes: 'Runs models on this computer. After installing, download a model with ollama pull llama3.2.', keywords: ['ollama', 'local llm', 'local models'] },
  { name: 'GitHub Copilot CLI', command: 'copilot', installCommand: 'npm install -g @github/copilot', docs: 'https://docs.github.com/en/copilot/how-tos/copilot-cli', notes: 'Needs a GitHub Copilot subscription. Sign in with /login the first time it runs.', keywords: ['copilot', 'github'] },
  { name: 'Open Interpreter', command: 'interpreter', installCommand: 'pip install open-interpreter', docs: 'https://docs.openinterpreter.com', notes: 'Lets a model run code on this computer. Choose a model provider after installing.', keywords: ['interpreter', 'open interpreter'] },
]

/** The four K2 can install itself, shown first when nothing is installed. */
const FEATURED = new Set(['claude', 'codex', 'grok', 'gemini'])

function cliManifest(): SettingEntry[] {
  return AGENT_TOOLS.map((t) => ({
    id: `agents.cli-${t.command}`,
    section: 'agents',
    group: 'Agent tools',
    label: t.name,
    description: `Whether ${t.name} is installed, and how to install it`,
    keywords: [...t.keywords, 'install', 'cli', 'tool'],
  }))
}

export const AGENTS_MANIFEST: SettingEntry[] = [
  {
    id: 'agents.default-agent',
    section: 'agents',
    group: 'Default agent',
    label: 'Default agent',
    description: defaultAgentLaunchCopy(),
    keywords: ['agent', 'default', 'claude', 'codex', 'gemini', 'llm', 'llms', 'models', 'language models'],
  },
  {
    id: 'agents.show-launch-bar',
    section: 'agents',
    group: 'Default agent',
    label: 'Show launch bar',
    description: 'The row of agent buttons under the tabs',
    keywords: ['launch', 'bar', 'strip', 'presets', 'hide', 'llm'],
  },
  {
    id: 'agents.agent-presets',
    section: 'agents',
    label: 'Launch presets',
    description: 'The agents on the launch bar and in the tab + menu',
    keywords: ['presets', 'commands', 'agents', 'agent presets'],
  },
  {
    id: 'agents.reset-built-ins',
    section: 'agents',
    label: 'Restore built-in presets',
    description: 'Put the presets K2 ships with back as they were',
    keywords: ['reset', 'restore', 'defaults', 'built-in', 'built-ins'],
  },
  {
    id: 'agents.add-preset',
    section: 'agents',
    label: 'Add preset',
    description: 'Add your own agent command to the launch bar',
    keywords: ['preset', 'custom', 'add', 'cli', 'command'],
  },
  {
    id: 'agents.submit-keys',
    section: 'agents',
    label: 'Message keys',
    description: 'How K2 types a message into an agent’s terminal',
    keywords: ['inject', 'paste', 'keystroke', 'submit', 'esc', 'return', 'keys'],
  },
  ...cliManifest(),
]

// The page follows Settings → Workspaces / Agents: a title with one line
// under it, then card groups of rows at one content width.
const CONTENT_WIDTH = 'max-w-[52rem]'

export function AgentsSection(): React.JSX.Element {
  const isRemote = useConnectHostStore((s) => s.activeHost !== 'local')
  const where = isRemote ? 'this server' : 'this computer'
  return (
    <div className="h-full min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]" data-testid="agents-page">
      <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`}>
        <header className="min-w-0">
          <h2 className="type-title text-[var(--color-text-primary)]">Providers / Models</h2>
          <p className="type-hint text-[var(--color-text-secondary)] mt-1">
            Choose the agent K2 opens, the agents on the launch bar, and the agent tools on {where}.
          </p>
        </header>
        <DefaultAgentGroup />
        <PresetsGroup />
        <AgentToolsGroup where={where} />
      </div>
    </div>
  )
}

// ── Default agent ───────────────────────────────────────────────────

function DefaultAgentGroup(): React.JSX.Element {
  const presets = usePresetsStore((s) => s.presets)
  const loaded = usePresetsStore((s) => s.presetsLoaded)
  const loadError = usePresetsStore((s) => s.presetsError)
  const showPresetsBar = usePresetsStore((s) => s.showPresetsBar)
  const setShowLaunchBar = usePresetsStore((s) => s.setShowLaunchBar)
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const setDefaultAgent = useSettingsStore((s) => s.setDefaultAgent)
  const [saveError, setSaveError] = useState<string | null>(null)

  const selected = matchAgentPreset(presets, defaultAgent)
  const options = presets.map((p) => ({
    value: p.id,
    label: p.label,
    leading: <PresetIcon preset={p} size={14} />,
    badge: p.enabled ? undefined : 'Off',
  }))

  const choose = async (id: string): Promise<void> => {
    setSaveError(null)
    await setDefaultAgent(id)
    // The store puts the old value back when the save fails.
    if (useSettingsStore.getState().defaultAgent !== id) {
      setSaveError('K2 couldn’t save the default agent, so it’s unchanged. Try again.')
    }
  }

  return (
    <SettingsGroup title="Default agent" card>
      <SettingRow
        label="Default agent"
        hint={defaultAgentLaunchCopy()}
        settingId="agents.default-agent"
        field
        below={saveError ? <p role="alert" className="type-hint text-[var(--color-status-error-text)]">{saveError}</p> : undefined}
      >
        <SettingDropdown
          fullWidth
          ariaLabel="Default agent"
          value={selected?.id ?? ''}
          options={options}
          placeholder={presets.length > 0 ? 'Choose an agent' : !loaded ? 'Loading…' : loadError ? 'Presets didn’t load' : 'No presets yet'}
          disabled={presets.length === 0}
          onChange={(id) => void choose(id)}
        />
      </SettingRow>
      <SettingRow
        label="Show launch bar"
        hint="The row of agent buttons under the tabs, one for each preset that’s on. Applies on this computer."
        settingId="agents.show-launch-bar"
      >
        <Toggle checked={showPresetsBar} onChange={setShowLaunchBar} aria-label="Show launch bar" />
      </SettingRow>
    </SettingsGroup>
  )
}

function PresetIcon({ preset, size = 16 }: { preset: Pick<AgentPreset, 'icon' | 'label'>; size?: number }): React.JSX.Element {
  return (
    <span className="w-5 flex items-center justify-center flex-shrink-0" aria-hidden="true">
      {preset.icon ? <span className="type-row leading-none">{preset.icon}</span> : <AgentIcon agent={preset.label} size={size} />}
    </span>
  )
}

// ── Launch presets ──────────────────────────────────────────────────

interface PresetFormState {
  /** null: a new preset. */
  editingId: string | null
  label: string
  command: string
  icon: string
  injectFlow: InjectFlowStep[]
  injectFlowTouched: boolean
}

const NEW_PRESET_FORM = (): PresetFormState => ({
  editingId: null,
  label: '',
  command: '',
  icon: '',
  injectFlow: cloneDefaultInjectFlow(),
  injectFlowTouched: false,
})

const INJECT_KEY_OPTIONS: { value: InjectFlowKey; label: string }[] = [
  { value: 'paste', label: 'Paste message' },
  { value: 'return', label: 'Return' },
  { value: 'esc', label: 'Esc' },
  { value: 'space', label: 'Space' },
]

const keyLabel = (key: InjectFlowKey): string => INJECT_KEY_OPTIONS.find((o) => o.value === key)?.label ?? key

/** "Paste message, then Return, then Return" — the steps in one line. */
export function injectFlowSummary(steps: InjectFlowStep[]): string {
  return steps.map((s) => keyLabel(s.key)).join(', then ')
}

const errorText = (err: unknown): string => (err instanceof Error && err.message ? err.message : String(err))

function PresetsGroup(): React.JSX.Element {
  const presets = usePresetsStore((s) => s.presets)
  const loaded = usePresetsStore((s) => s.presetsLoaded)
  const loadError = usePresetsStore((s) => s.presetsError)
  const fetchPresets = usePresetsStore((s) => s.fetchPresets)
  const updatePreset = usePresetsStore((s) => s.updatePreset)
  const deletePreset = usePresetsStore((s) => s.deletePreset)
  const reorderPresets = usePresetsStore((s) => s.reorderPresets)
  const [form, setForm] = useState<PresetFormState | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [savedId, setSavedId] = useState<string | null>(null)
  const [dragIdx, setDragIdx] = useState<number | null>(null)
  const [dragOverIdx, setDragOverIdx] = useState<number | null>(null)
  const dragFromRef = useRef<number | null>(null)
  const dropRef = useRef<number | null>(null)
  const addButtonRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    void fetchPresets()
  }, [fetchPresets])

  useEffect(() => {
    if (!savedId) return
    const t = window.setTimeout(() => setSavedId(null), 2500)
    return () => window.clearTimeout(t)
  }, [savedId])

  const run = useCallback(async (what: string, fn: () => Promise<void>): Promise<boolean> => {
    setActionError(null)
    try {
      await fn()
      return true
    } catch (err) {
      setActionError(`${what} ${errorText(err)}`)
      return false
    }
  }, [])

  const toggle = (p: AgentPreset): void => {
    void run(`K2 couldn’t turn ${p.label} ${p.enabled ? 'off' : 'on'}.`, () => updatePreset({ id: p.id, enabled: p.enabled ? 0 : 1 }))
  }

  const move = (from: number, to: number): void => {
    if (to < 0 || to >= presets.length || from === to) return
    const ids = presets.map((p) => p.id)
    const [moved] = ids.splice(from, 1)
    ids.splice(to, 0, moved)
    void run('K2 couldn’t change the order.', () => reorderPresets(ids))
  }

  const remove = async (p: AgentPreset): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Delete ${p.label}?`,
      message: `${p.label} leaves the launch bar and the tab + menu. This can’t be undone.`,
      confirmLabel: 'Delete',
      destructive: true,
    })
    if (!ok) return
    await run(`K2 couldn’t delete ${p.label}.`, () => deletePreset(p.id))
  }

  const edit = (p: AgentPreset): void => {
    setActionError(null)
    setForm({
      editingId: p.id,
      label: p.label,
      command: p.command,
      icon: p.icon ?? '',
      injectFlow: parseInjectFlowOrDefault(readPresetInjectFlowJson(p), p.command),
      injectFlowTouched: false,
    })
  }

  const closeForm = (savedPresetId?: string): void => {
    const wasNew = form?.editingId === null
    setForm(null)
    if (savedPresetId) setSavedId(savedPresetId)
    if (wasNew && !savedPresetId) requestAnimationFrame(() => addButtonRef.current?.focus())
  }

  const onGripMouseDown = (e: React.MouseEvent, idx: number): void => {
    if (e.button !== 0) return
    e.preventDefault()
    const startY = e.clientY
    let started = false
    const onMove = (ev: MouseEvent): void => {
      if (!started && Math.abs(ev.clientY - startY) > 4) {
        started = true
        dragFromRef.current = idx
        setDragIdx(idx)
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }
      if (!started) return
      const items = document.querySelectorAll('[data-preset-reorder-index]')
      let drop = 0
      for (let i = 0; i < items.length; i++) {
        const r = items[i].getBoundingClientRect()
        if (ev.clientY > r.top + r.height / 2) drop = i + 1
      }
      dropRef.current = drop
      setDragOverIdx(drop)
    }
    const onUp = (): void => {
      document.removeEventListener('mousemove', onMove)
      document.removeEventListener('mouseup', onUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
      const from = dragFromRef.current
      const drop = dropRef.current
      if (started && from !== null && drop !== null && from !== drop && from !== drop - 1) {
        move(from, drop > from ? drop - 1 : drop)
      }
      setDragIdx(null)
      setDragOverIdx(null)
      dragFromRef.current = null
      dropRef.current = null
    }
    document.addEventListener('mousemove', onMove)
    document.addEventListener('mouseup', onUp)
  }

  const nothingYet = presets.length === 0
  const adding = form?.editingId === null

  let body: React.ReactNode
  if (nothingYet && !loaded) {
    body = (
      <p role="status" className="px-4 py-3 type-row text-[var(--color-text-secondary)]" data-testid="presets-loading">
        Loading your presets…
      </p>
    )
  } else if (nothingYet && loadError) {
    body = (
      <div className="px-4 py-3 flex items-center justify-between gap-4" data-testid="presets-load-error" role="alert">
        <div className="min-w-0">
          <div className="type-row text-[var(--color-text-primary)]">Couldn’t load your presets</div>
          <div className={SETTING_ROW_HINT} title={loadError}>
            K2 couldn’t read your presets, so the launch bar may be empty too. Try again in a moment.
          </div>
        </div>
        <Button variant="quiet" className="flex-shrink-0" onClick={() => void fetchPresets()}>
          Try again
        </Button>
      </div>
    )
  } else if (nothingYet && !adding) {
    body = (
      <div className="px-4 py-3 flex items-center justify-between gap-4" data-testid="presets-empty">
        <div className="min-w-0">
          <div className="type-row text-[var(--color-text-primary)]">No presets yet</div>
          <div className={SETTING_ROW_HINT}>Restore the built-in presets for Claude, Codex, Grok, Gemini and more, or add your own.</div>
        </div>
        <RestoreBuiltInsButton onError={setActionError} />
      </div>
    )
  } else {
    body = (
      <>
        {adding && form && <PresetForm form={form} setForm={setForm} onDone={closeForm} />}
        <div data-preset-reorder-container className="divide-y divide-[var(--color-border)]">
          {presets.map((p, i) =>
            form?.editingId === p.id ? (
              <PresetForm key={p.id} form={form} setForm={setForm} onDone={closeForm} />
            ) : (
              <PresetRow
                key={p.id}
                preset={p}
                index={i}
                count={presets.length}
                dragging={dragIdx === i}
                dropBefore={dragOverIdx === i}
                dropAfter={dragOverIdx === presets.length && i === presets.length - 1}
                saved={savedId === p.id}
                onGripMouseDown={(e) => onGripMouseDown(e, i)}
                onToggle={() => toggle(p)}
                onEdit={() => edit(p)}
                onDelete={() => void remove(p)}
                onMove={(dir) => move(i, i + dir)}
              />
            ),
          )}
        </div>
        {!nothingYet && (
          <SettingRow
            label="Built-in presets"
            hint="Put the presets K2 ships with back as they were: their commands, icons and keys, all on. Presets you added stay."
            settingId="agents.reset-built-ins"
          >
            <RestoreBuiltInsButton onError={setActionError} />
          </SettingRow>
        )}
      </>
    )
  }

  return (
    <SettingsGroup
      title="Launch presets"
      hint="Each preset that’s on is a button on the launch bar and a choice in the tab + menu. Drag to change the order."
      settingId="agents.agent-presets"
      card
      action={
        loaded && !(nothingYet && loadError) && !adding ? (
          <Button
            ref={addButtonRef}
            variant="accent"
            data-settings-id="agents.add-preset"
            onClick={() => {
              setActionError(null)
              setForm(NEW_PRESET_FORM())
            }}
          >
            Add preset
          </Button>
        ) : undefined
      }
    >
      {actionError && (
        <div className="px-4 py-3">
          <Callout tone="error" role="alert" className="flex items-start justify-between gap-4">
            <p className="type-hint min-w-0 break-words">{actionError}</p>
            <Button variant="quiet" className="flex-shrink-0" onClick={() => setActionError(null)}>
              Dismiss
            </Button>
          </Callout>
        </div>
      )}
      {loadError && !nothingYet && (
        <div className="px-4 py-3 flex items-center justify-between gap-4" role="alert" data-testid="presets-refresh-error">
          <div className={SETTING_ROW_HINT}>K2 couldn’t refresh this list, so it may be out of date.</div>
          <Button variant="quiet" className="flex-shrink-0" onClick={() => void fetchPresets()}>
            Try again
          </Button>
        </div>
      )}
      {body}
    </SettingsGroup>
  )
}

function RestoreBuiltInsButton({ onError }: { onError: (msg: string | null) => void }): React.JSX.Element {
  const resetPresetsToBuiltIns = usePresetsStore((s) => s.resetPresetsToBuiltIns)
  const [state, setState] = useState<'idle' | 'working' | 'done'>('idle')

  useEffect(() => {
    if (state !== 'done') return
    const t = window.setTimeout(() => setState('idle'), 2500)
    return () => window.clearTimeout(t)
  }, [state])

  const restore = async (): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: 'Restore the built-in presets?',
      message:
        'Claude, Codex, Grok, Gemini and the other built-in presets go back to how K2 ships them, all on. Changes you made to them are lost. Presets you added stay as they are.',
      confirmLabel: 'Restore',
    })
    if (!ok) return
    onError(null)
    setState('working')
    try {
      await resetPresetsToBuiltIns()
      setState('done')
    } catch (err) {
      setState('idle')
      onError(`K2 couldn’t restore the built-in presets. ${errorText(err)}`)
    }
  }

  return (
    <>
      {state === 'done' && (
        <span role="status" className="type-hint text-[var(--color-status-ok-soft)]">
          Restored
        </span>
      )}
      <Button variant="quiet" className="flex-shrink-0" disabled={state === 'working'} onClick={() => void restore()}>
        {state === 'working' ? 'Restoring…' : 'Restore…'}
      </Button>
    </>
  )
}

function GripIcon(): React.JSX.Element {
  return (
    <svg width="8" height="12" viewBox="0 0 8 12" fill="currentColor" aria-hidden="true" className="block">
      {[2, 6, 10].map((y) => (
        <React.Fragment key={y}>
          <circle cx="2" cy={y} r="1" />
          <circle cx="6" cy={y} r="1" />
        </React.Fragment>
      ))}
    </svg>
  )
}

function PresetRow({
  preset,
  index,
  count,
  dragging,
  dropBefore,
  dropAfter,
  saved,
  onGripMouseDown,
  onToggle,
  onEdit,
  onDelete,
  onMove,
}: {
  preset: AgentPreset
  index: number
  count: number
  dragging: boolean
  dropBefore: boolean
  dropAfter: boolean
  saved: boolean
  onGripMouseDown: (e: React.MouseEvent) => void
  onToggle: () => void
  onEdit: () => void
  onDelete: () => void
  onMove: (dir: -1 | 1) => void
}): React.JSX.Element {
  const on = Boolean(preset.enabled)
  return (
    <div
      data-preset-reorder-index={index}
      data-testid={`preset-row-${preset.id}`}
      className={`relative flex items-center gap-3 px-4 py-2.5 hover:bg-[var(--color-wash-1)] transition-colors ${dragging ? 'opacity-40' : ''}`}
    >
      {dropBefore && <div className="absolute left-0 right-0 top-0 h-0.5 bg-[var(--color-accent)] z-10" />}
      {dropAfter && <div className="absolute left-0 right-0 bottom-0 h-0.5 bg-[var(--color-accent)] z-10" />}
      {/* The grip sits in the row's left padding, so the icon lines up with
          every other row's start. */}
      <span
        className="absolute left-0 top-0 bottom-0 w-4 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] cursor-grab active:cursor-grabbing"
        title="Drag to change the order"
        onMouseDown={onGripMouseDown}
      >
        <GripIcon />
      </span>
      <PresetIcon preset={preset} />
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2 min-w-0">
          <span className={`type-name truncate ${on ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]'}`}>
            {preset.label}
          </span>
          {!preset.isBuiltIn && <Tag tone="muted">Custom</Tag>}
          {saved && (
            <span role="status" className="type-hint text-[var(--color-status-ok-soft)]">
              Saved
            </span>
          )}
        </div>
        <code className="block type-hint font-mono text-[var(--color-text-secondary)] truncate mt-0.5" title={preset.command}>
          {preset.command}
        </code>
      </div>
      <Toggle
        checked={on}
        onChange={onToggle}
        aria-label={`Show ${preset.label} on the launch bar`}
        title={on ? `${preset.label} is on the launch bar` : `${preset.label} is hidden`}
      />
      <OverflowMenu
        label={`More actions for ${preset.label}`}
        items={[
          { id: 'edit', label: 'Edit…', onSelect: onEdit },
          { id: 'up', label: 'Move up', onSelect: () => onMove(-1), disabled: index === 0, group: true },
          { id: 'down', label: 'Move down', onSelect: () => onMove(1), disabled: index === count - 1 },
          ...(preset.isBuiltIn
            ? []
            : [{ id: 'delete', label: 'Delete…', onSelect: onDelete, danger: true, group: true }]),
        ]}
      />
    </div>
  )
}

function PresetForm({
  form,
  setForm,
  onDone,
}: {
  form: PresetFormState
  setForm: React.Dispatch<React.SetStateAction<PresetFormState | null>>
  onDone: (savedId?: string) => void
}): React.JSX.Element {
  const createPreset = usePresetsStore((s) => s.createPreset)
  const updatePreset = usePresetsStore((s) => s.updatePreset)
  const nameRef = useRef<HTMLInputElement>(null)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [keysOpen, setKeysOpen] = useState(form.injectFlowTouched)
  const editing = form.editingId !== null
  const valid = form.label.trim() !== '' && form.command.trim() !== ''

  useEffect(() => {
    requestAnimationFrame(() => nameRef.current?.focus())
  }, [])

  const patch = (p: Partial<PresetFormState>): void => setForm((s) => (s ? { ...s, ...p } : s))

  const save = async (): Promise<void> => {
    if (!valid || saving) return
    setSaving(true)
    setError(null)
    try {
      const injectPayload = form.injectFlowTouched
        ? isDefaultInjectFlowForCommand(form.injectFlow, form.command)
          ? ''
          : JSON.stringify(form.injectFlow)
        : undefined
      let id: string
      if (form.editingId) {
        id = form.editingId
        await updatePreset({
          id,
          label: form.label.trim(),
          command: form.command.trim(),
          icon: form.icon.trim() || '',
          ...(injectPayload !== undefined ? { injectFlow: injectPayload } : {}),
        })
      } else {
        const created = await createPreset({
          label: form.label.trim(),
          command: form.command.trim(),
          icon: form.icon.trim() || undefined,
        })
        id = created.id
        if (injectPayload) await updatePreset({ id, injectFlow: injectPayload })
      }
      onDone(id)
    } catch (err) {
      setError(`K2 couldn’t save this preset. ${errorText(err)}`)
      setSaving(false)
    }
  }

  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key === 'Escape') {
      // Claimed, so Settings doesn't also close.
      e.preventDefault()
      e.stopPropagation()
      onDone()
      return
    }
    if (e.key !== 'Enter') return
    const t = e.target as HTMLElement
    if (t.tagName !== 'INPUT' || t.dataset.injectWait !== undefined) return
    e.preventDefault()
    void save()
  }

  return (
    <div
      className="bg-[var(--color-wash-1)] divide-y divide-[var(--color-border)]"
      onKeyDown={onKeyDown}
      data-testid="preset-form"
      role="group"
      aria-label={editing ? `Edit ${form.label || 'preset'}` : 'New preset'}
    >
      <div className="px-4 pt-3 pb-2">
        <div className="type-name text-[var(--color-text-primary)]">{editing ? 'Edit preset' : 'New preset'}</div>
      </div>
      <SettingRow label="Name" hint="Shown on the launch bar and in the tab + menu." field>
        <Input ref={nameRef} aria-label="Name" value={form.label} placeholder="e.g. Aider with GPT" onChange={(e) => patch({ label: e.target.value })} className="w-full type-control" />
      </SettingRow>
      <SettingRow label="Command" hint="What K2 runs in a new terminal, with any options." field>
        <Input aria-label="Command" value={form.command} placeholder="e.g. aider --model gpt-4o" onChange={(e) => patch({ command: e.target.value })} className="w-full type-control font-mono" spellCheck={false} />
      </SettingRow>
      <SettingRow label="Icon" hint="An emoji. Leave it empty to use the agent’s logo." field>
        <Input aria-label="Icon" value={form.icon} placeholder="Agent’s logo" onChange={(e) => patch({ icon: e.target.value })} className="w-full type-control" />
      </SettingRow>
      <SettingRow
        label="Message keys"
        hint={`How K2 types a message into this agent: ${injectFlowSummary(form.injectFlow)}.`}
        settingId="agents.submit-keys"
        below={
          keysOpen ? (
            <SubmitKeysEditor
              steps={form.injectFlow}
              command={form.command}
              onChange={(injectFlow) => patch({ injectFlow, injectFlowTouched: true })}
            />
          ) : undefined
        }
      >
        <Button variant="quiet" aria-expanded={keysOpen} onClick={() => setKeysOpen((v) => !v)}>
          {keysOpen ? 'Done' : 'Change'}
        </Button>
      </SettingRow>
      <div className="px-4 py-3 space-y-2">
        {error && (
          <Callout tone="error" role="alert">
            <p className="type-hint break-words">{error}</p>
          </Callout>
        )}
        <div className="flex items-center justify-end gap-2">
          {!valid && <span className="type-hint text-[var(--color-text-secondary)] mr-auto">Enter a name and a command to save.</span>}
          <Button variant="quiet" onClick={() => onDone()}>
            Cancel
          </Button>
          <Button variant="accent" disabled={!valid || saving} onClick={() => void save()}>
            {saving ? 'Saving…' : editing ? 'Save' : 'Add preset'}
          </Button>
        </div>
      </div>
    </div>
  )
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
  const setStep = (i: number, p: Partial<InjectFlowStep>): void => onChange(steps.map((s, j) => (j === i ? { ...s, ...p } : s)))
  const remove = (i: number): void => {
    if (steps.length > 1) onChange(steps.filter((_, j) => j !== i))
  }
  const move = (i: number, dir: -1 | 1): void => {
    const j = i + dir
    if (j < 0 || j >= steps.length) return
    const next = [...steps]
    ;[next[i], next[j]] = [next[j], next[i]]
    onChange(next)
  }
  const add = (): void => {
    if (steps.length >= 16) return
    const hasPaste = steps.some((s) => s.key === 'paste')
    onChange([...steps, { key: hasPaste ? 'return' : 'paste', waitMs: 150 }])
  }

  return (
    <Surface chamfer material="tile" className="p-3 space-y-3">
      <p className={SETTING_ROW_HINT}>
        Paste message writes the text without the clipboard. Return sends it. Esc and Space are extra keys some agents need. The wait is how long K2 pauses after that step.
      </p>
      <ol className="space-y-2">
        {steps.map((step, i) => (
          <li key={i} className="flex items-center gap-2">
            <span className="w-5 flex-shrink-0 type-hint text-[var(--color-text-secondary)] text-right tabular-nums" aria-hidden="true">
              {i + 1}.
            </span>
            <div className="w-40 flex-shrink-0">
              <SettingDropdown
                value={step.key}
                options={INJECT_KEY_OPTIONS}
                onChange={(key) => setStep(i, { key: key as InjectFlowKey })}
                menuAlign="left"
                ariaLabel={`Step ${i + 1} key`}
                fullWidth
              />
            </div>
            <span className="type-hint text-[var(--color-text-secondary)] whitespace-nowrap">then wait</span>
            <Input
              type="number"
              min={0}
              max={10000}
              data-inject-wait=""
              aria-label={`Step ${i + 1} wait in milliseconds`}
              value={step.waitMs}
              onChange={(e) => {
                const n = Number.parseInt(e.target.value, 10)
                setStep(i, { waitMs: Number.isFinite(n) ? Math.max(0, Math.min(10000, n)) : 0 })
              }}
              className="w-20 type-control font-mono tabular-nums"
            />
            <span className="type-hint text-[var(--color-text-secondary)]">ms</span>
            <span className="flex items-center gap-1 ml-auto">
              <Button variant="quiet" className="!px-2" onClick={() => move(i, -1)} disabled={i === 0} aria-label={`Move step ${i + 1} up`}>
                ↑
              </Button>
              <Button variant="quiet" className="!px-2" onClick={() => move(i, 1)} disabled={i === steps.length - 1} aria-label={`Move step ${i + 1} down`}>
                ↓
              </Button>
              <Button variant="quiet" className="!px-2" onClick={() => remove(i)} disabled={steps.length <= 1} aria-label={`Remove step ${i + 1}`}>
                ×
              </Button>
            </span>
          </li>
        ))}
      </ol>
      <div className="flex items-center gap-2">
        <Button variant="quiet" onClick={add} disabled={steps.length >= 16}>
          Add step
        </Button>
        <Button variant="quiet" onClick={() => onChange(cloneDefaultInjectFlowForCommand(command))}>
          Use the default
        </Button>
      </div>
    </Surface>
  )
}

// ── Agent tools on this computer ────────────────────────────────────
// Which tools are on the server's PATH comes from the daemon
// (GET agents/cli-status). A server without that route says so instead of
// guessing; install help works either way.

interface CliStatus {
  program: string
  installed: boolean
  version?: string | null
}

type ToolsState =
  | { kind: 'loading' }
  | { kind: 'ready'; byProgram: Map<string, CliStatus> }
  | { kind: 'unsupported' }
  | { kind: 'error'; message: string }

function isMissingRoute(message: string): boolean {
  return /route_unclassified|not found|\b404\b/i.test(message)
}

function useCliStatus(): [ToolsState, () => void] {
  const [state, setState] = useState<ToolsState>({ kind: 'loading' })
  const [nonce, setNonce] = useState(0)
  useEffect(() => {
    let live = true
    setState({ kind: 'loading' })
    daemonCliGet<{ tools?: CliStatus[] }>(primaryScope(), 'agents/cli-status')
      .then((r) => {
        if (!live) return
        const tools = Array.isArray(r?.tools) ? r.tools : null
        if (!tools) {
          setState({ kind: 'unsupported' })
          return
        }
        setState({ kind: 'ready', byProgram: new Map(tools.map((t) => [t.program, t])) })
      })
      .catch((err: unknown) => {
        if (!live) return
        const message = errorText(err)
        setState(isMissingRoute(message) ? { kind: 'unsupported' } : { kind: 'error', message })
      })
    return () => {
      live = false
    }
  }, [nonce])
  return [state, useCallback(() => setNonce((n) => n + 1), [])]
}

function openExternal(url: string): void {
  invoke('plugin:opener|open_url', { url }).catch(() => {
    window.open(url, '_blank', 'noopener')
  })
}

function AgentToolsGroup({ where }: { where: string }): React.JSX.Element {
  const [state, recheck] = useCliStatus()
  const [openTool, setOpenTool] = useState<string | null>(null)
  const [showMore, setShowMore] = useState(false)

  const ready = state.kind === 'ready' ? state.byProgram : null
  const installed = ready ? AGENT_TOOLS.filter((t) => ready.get(t.command)?.installed) : []
  const first = ready && installed.length > 0 ? installed : AGENT_TOOLS.filter((t) => FEATURED.has(t.command))
  const firstSet = new Set(first.map((t) => t.command))
  const more = AGENT_TOOLS.filter((t) => !firstSet.has(t.command))
  const moreLabel = ready && installed.length > 0 ? 'More agents you can install' : 'More agents'

  const row = (t: AgentTool): React.JSX.Element => (
    <ToolRow
      key={t.command}
      tool={t}
      status={ready ? (ready.get(t.command) ?? { program: t.command, installed: false }) : null}
      where={where}
      open={openTool === t.command}
      onToggle={() => setOpenTool((cur) => (cur === t.command ? null : t.command))}
      onInstalled={recheck}
    />
  )

  let notice: React.ReactNode = null
  if (state.kind === 'loading') {
    notice = (
      <p role="status" className="px-4 py-3 type-row text-[var(--color-text-secondary)]" data-testid="tools-loading">
        Checking which agent tools are on {where}…
      </p>
    )
  } else if (state.kind === 'error') {
    notice = (
      <div className="px-4 py-3 flex items-center justify-between gap-4" role="alert" data-testid="tools-error">
        <div className="min-w-0">
          <div className="type-row text-[var(--color-text-primary)]">Couldn’t check which tools are installed</div>
          <div className={SETTING_ROW_HINT} title={state.message}>
            The install steps below still work. Try again in a moment.
          </div>
        </div>
        <Button variant="quiet" className="flex-shrink-0" onClick={recheck}>
          Try again
        </Button>
      </div>
    )
  } else if (state.kind === 'unsupported') {
    notice = (
      <p className={`px-4 py-3 ${SETTING_ROW_HINT}`} data-testid="tools-unsupported">
        This version of K2 can’t check which tools are installed on {where}. Open a tool to see how to install it.
      </p>
    )
  } else if (installed.length === 0) {
    notice = (
      <div className="px-4 py-3" data-testid="tools-none">
        <div className="type-row text-[var(--color-text-primary)]">No agent tools on {where} yet</div>
        <div className={SETTING_ROW_HINT}>Install one to start an agent. K2 can install the first four for you.</div>
      </div>
    )
  }

  return (
    <SettingsGroup
      title={`Agent tools on ${where}`}
      hint="The programs your presets run. Install one here, then turn its preset on."
      card
      action={
        state.kind === 'ready' ? (
          <Button variant="quiet" onClick={recheck}>
            Check again
          </Button>
        ) : undefined
      }
    >
      {notice}
      {state.kind !== 'loading' && first.map(row)}
      {state.kind !== 'loading' && (
        <div className="relative">
          {/* Search lands here for a tool that's folded away. */}
          {!showMore && more.map((t) => <span key={t.command} data-settings-id={`agents.cli-${t.command}`} className="absolute inset-0 pointer-events-none" />)}
          <button
            type="button"
            data-k2-focus=""
            aria-expanded={showMore}
            onClick={() => setShowMore((v) => !v)}
            className="relative w-full flex items-center justify-between gap-4 px-4 py-3 text-left hover:bg-[var(--color-wash-1)] transition-colors no-drag cursor-pointer"
          >
            <span className="min-w-0">
              <span className="block type-row text-[var(--color-text-primary)]">{moreLabel}</span>
              <span className={`block ${SETTING_ROW_HINT}`}>
                {more.length > 3 ? `${more.slice(0, 3).map((t) => t.name).join(', ')} and ${more.length - 3} more` : more.map((t) => t.name).join(', ')}
              </span>
            </span>
            <span className="flex items-center gap-2 flex-shrink-0 type-hint text-[var(--color-text-secondary)]">
              {showMore ? 'Hide' : `Show ${more.length}`}
              <svg className={`w-3 h-3 transition-transform ${showMore ? 'rotate-180' : ''}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2} aria-hidden="true">
                <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
              </svg>
            </span>
          </button>
        </div>
      )}
      {state.kind !== 'loading' && showMore && more.map(row)}
    </SettingsGroup>
  )
}

function ToolRow({
  tool,
  status,
  where,
  open,
  onToggle,
  onInstalled,
}: {
  tool: AgentTool
  /** null: K2 doesn't know (still checking, or the server can't say). */
  status: CliStatus | null
  where: string
  open: boolean
  onToggle: () => void
  onInstalled: () => void
}): React.JSX.Element {
  const installing = useCliInstallStore((s) => s.installing)
  const [copied, setCopied] = useState(false)
  const [installError, setInstallError] = useState<string | null>(null)
  const [justInstalled, setJustInstalled] = useState(false)
  const program = isInstallableCli(tool.command) ? tool.command : null
  const isInstalling = program !== null && installing === program
  const isInstalled = status?.installed === true

  const copy = (): void => {
    navigator.clipboard?.writeText(tool.installCommand).then(
      () => {
        setCopied(true)
        window.setTimeout(() => setCopied(false), 2000)
      },
      () => setInstallError('K2 couldn’t copy the command. Select it and copy it by hand.'),
    )
  }

  const install = (p: InstallableCli): void => {
    setInstallError(null)
    setJustInstalled(false)
    ensureOneCli(primaryScope(), p)
      .then(() => {
        setJustInstalled(true)
        onInstalled()
      })
      .catch((err: unknown) => setInstallError(errorText(err)))
  }

  const statusLine = status === null
    ? 'Open to see how to install it.'
    : isInstalled
      ? status.version
        ? `Version ${status.version}`
        : 'Version unknown'
      : program
        ? 'Not installed. K2 installs it the first time you start it.'
        : 'Not installed'

  return (
    <SettingRow
      label={
        <span className="flex items-center gap-2 min-w-0">
          <span className="w-5 flex items-center justify-center flex-shrink-0" aria-hidden="true">
            <AgentIcon agent={tool.command} size={16} />
          </span>
          <span className="truncate">{tool.name}</span>
          {isInstalled && <Tag tone="ok">Installed</Tag>}
        </span>
      }
      hint={<span className="block pl-7">{statusLine}</span>}
      settingId={`agents.cli-${tool.command}`}
      testId={`tool-row-${tool.command}`}
      below={
        open ? (
          <div className="pl-7 space-y-3" data-testid={`tool-help-${tool.command}`}>
            <div className="flex items-start gap-2">
              <code className="flex-1 min-w-0 break-all select-text type-hint font-mono px-2 py-1.5 rounded-[var(--radius-field)] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)]">
                {tool.installCommand}
              </code>
              <Button variant="quiet" className="flex-shrink-0" onClick={copy}>
                {copied ? 'Copied' : 'Copy'}
              </Button>
            </div>
            <p className={SETTING_ROW_HINT}>
              {program && !isInstalled
                ? `Install runs this command on ${where}. Or copy it into a terminal. `
                : `Run this in a terminal on ${where}${isInstalled ? ' to update it' : ''}. `}
              {tool.notes}
            </p>
            {isInstalling && program && (
              <p role="status" className="type-hint text-[var(--color-text-secondary)]">
                {installingCliLabel(program)} This can take a minute or two.
              </p>
            )}
            {justInstalled && !isInstalling && (
              <p role="status" className="type-hint text-[var(--color-status-ok-soft)]">
                {tool.name} is installed. Turn its preset on to put it on the launch bar.
              </p>
            )}
            {installError && (
              <Callout tone="error" role="alert">
                <p className="type-hint break-words">K2 couldn’t install {tool.name}. {installError}</p>
              </Callout>
            )}
            <div className="flex items-center gap-2">
              {program && !isInstalled && (
                <Button variant="accent" disabled={isInstalling} onClick={() => install(program)}>
                  {isInstalling ? installingCliLabel(program) : installError ? 'Try again' : `Install ${tool.name}`}
                </Button>
              )}
              <Button variant="quiet" onClick={() => openExternal(tool.docs)}>
                Open the {tool.name} docs
              </Button>
            </div>
          </div>
        ) : undefined
      }
    >
      <Button variant="quiet" aria-expanded={open} aria-label={`${open ? 'Hide' : 'Show'} how to install ${tool.name}`} onClick={onToggle}>
        {open ? 'Hide' : isInstalled ? 'Details' : 'How to install'}
      </Button>
    </SettingRow>
  )
}
