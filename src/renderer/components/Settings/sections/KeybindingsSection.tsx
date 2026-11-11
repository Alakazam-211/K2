import React from 'react'
import { useEffect, useRef, useState } from 'react'
import { useSettingsStore, getEffectiveKeybinding } from '@/stores/settings'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import type { ShortcutModifierLayout } from '@/stores/terminal-settings'
import {
  HOTKEYS,
  formatKeyCombo,
  keyEventToCombo,
  isReservedKey,
} from '@shared/hotkeys'
import type { HotkeyDefinition } from '@shared/hotkeys'
import { KeyCombo } from '@/components/KeySymbol'
import { Button, cx } from '@/components/ui'
import { isMacPlatform } from '@/lib/desktop-chrome'
import type { SettingEntry } from '../searchManifest'
import { ZEN_CHORD_COMBO, ZEN_CHORD_LABEL } from '@/lib/zen/zen-shortcut'
import { zenAvailable } from '@/lib/zen/zen-platform'
import {
  SaveError,
  SaveNote,
  SETTING_ROW_HINT,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  useSaveStatus,
} from '../controls/SettingControls'
import { KEY_CHIP, KEY_CHIP_REST, KeyChip } from '../shared/KeyChip'
import { AccessSheet, FailedNote } from './AccessDialogs'

/** prd-zen-mode-v1 Z30/Z56: the Zen escape chord is fixed (the native menu
 *  accelerator on macOS, a capture listener elsewhere). Shown here as taken;
 *  never rebindable, and no other binding may take it. */
export function isZenChordCombo(combo: string): boolean {
  return combo === ZEN_CHORD_COMBO.mac || combo === ZEN_CHORD_COMBO.other
}

/**
 * Keybindings manifest: one entry per hotkey definition (dynamically
 * derived from HOTKEYS) plus the workspace-number-shortcuts layout
 * toggle. HOTKEYS is imported above, so changing it here once keeps
 * the search index in sync.
 */
export const KEYBINDINGS_MANIFEST: SettingEntry[] = [
  {
    id: 'keybindings.shortcut-layout',
    section: 'keybindings',
    label: 'Number keys open',
    description: 'Which workspaces ⌘ 1–9 and ⌥⌘ 1–9 switch to',
    keywords: ['workspace', 'shortcut', 'layout', 'number', 'cmd', 'pinned', 'active', 'swap'],
  },
  {
    id: 'keybindings.reset-all',
    section: 'keybindings',
    label: 'Reset all shortcuts',
    description: 'Puts every shortcut back to its default',
    keywords: ['reset', 'defaults', 'keybindings', 'shortcuts'],
  },
  ...HOTKEYS.map((h) => ({
    id: `keybindings.${h.id}`,
    section: 'keybindings' as const,
    group: h.category,
    label: h.label,
    description: `Change the shortcut for ${h.label}`,
    keywords: [h.id, h.category.toLowerCase(), 'hotkey', 'shortcut', 'keybinding', 'rebind'],
  })),
]

const CATEGORIES = ['Terminal', 'Tabs', 'Navigation', 'App'] as const
const ACTIVE_FIRST: ShortcutModifierLayout = 'cmd-active-cmdshift-pinned'

function keysFor(combo: string): string {
  return formatKeyCombo(combo, isMacPlatform())
}

export function KeybindingsSection(): React.JSX.Element {
  const keybindings = useSettingsStore((s) => s.keybindings)
  const updateKeybinding = useSettingsStore((s) => s.updateKeybinding)
  const resetKeybinding = useSettingsStore((s) => s.resetKeybinding)
  const shortcutLayout = useTerminalSettingsStore((s) => s.shortcutLayout)
  const setShortcutLayout = useTerminalSettingsStore((s) => s.setShortcutLayout)
  const [capturing, setCapturing] = useState<string | null>(null)
  const [confirmReset, setConfirmReset] = useState(false)
  const [resetNote, setResetNote] = useState<string | null>(null)
  const layoutSave = useSaveStatus()
  const mac = isMacPlatform()

  // Which shortcuts share keys.
  const comboToIds: Record<string, string[]> = {}
  for (const h of HOTKEYS) {
    const combo = getEffectiveKeybinding(keybindings, h.id)
    if (!comboToIds[combo]) comboToIds[combo] = []
    comboToIds[combo].push(h.id)
  }
  const clashesWith = (id: string): string[] => {
    const combo = getEffectiveKeybinding(keybindings, id)
    return (comboToIds[combo] ?? []).filter((o) => o !== id).map((o) => HOTKEYS.find((h) => h.id === o)?.label ?? o)
  }

  const changed = HOTKEYS.filter((h) => !!keybindings[h.id]).length
  const activeFirst = shortcutLayout === ACTIVE_FIRST
  const numberKey = mac ? '⌘ 1–9' : 'Ctrl+1–9'
  const altNumberKey = mac ? '⌥⌘ 1–9' : 'Ctrl+Alt+1–9'

  return (
    <div className="w-full max-w-[52rem] space-y-6 pb-6" data-testid="keybindings-page">
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)]">Key Bindings</h2>
        <p className={SETTING_ROW_HINT}>
          Shortcuts for K2 on this computer. Click a shortcut and press the new keys; it saves at once. Escape cancels.
        </p>
      </header>

      <SettingsGroup card title="Workspace numbers" testId="keys-layout">
        <SettingRow
          settingId="keybindings.shortcut-layout"
          label="Number keys open"
          hint={
            <>
              Which workspaces {numberKey} and {altNumberKey} switch to, by their place in the sidebar.{' '}
              <SaveNote state={layoutSave.state} />
            </>
          }
          field
        >
          <SettingDropdown
            fullWidth
            ariaLabel="Number keys open"
            value={shortcutLayout}
            options={[
              { value: 'cmd-active-cmdshift-pinned', label: `${numberKey} Active, ${altNumberKey} Pinned` },
              { value: 'cmd-pinned-cmdshift-active', label: `${numberKey} Pinned, ${altNumberKey} Active` },
            ]}
            onChange={(v) => {
              setShortcutLayout(v as ShortcutModifierLayout)
              layoutSave.saved()
            }}
          />
        </SettingRow>
      </SettingsGroup>

      {CATEGORIES.map((category) => (
        <SettingsGroup key={category} card title={category} testId={`keys-${category.toLowerCase()}`}>
          {category === 'Navigation' && (
            <>
              <SettingRow label="Active workspaces" hint="Set by Number keys open, above.">
                <KeyChip combo={activeFirst ? numberKey : altNumberKey} />
              </SettingRow>
              <SettingRow label="Pinned workspaces" hint="Set by Number keys open, above.">
                <KeyChip combo={activeFirst ? altNumberKey : numberKey} />
              </SettingRow>
            </>
          )}
          {category === 'App' && zenAvailable() && (
            <div data-zen-keybinding="">
              <SettingRow label="Enter or leave Zen mode" hint="Always this shortcut, so you can get back from a Garden.">
                <KeyChip combo={mac ? ZEN_CHORD_LABEL.mac : ZEN_CHORD_LABEL.other} />
              </SettingRow>
            </div>
          )}
          {HOTKEYS.filter((h) => h.category === category).map((hotkey) => (
            <KeybindingRow
              key={hotkey.id}
              hotkey={hotkey}
              combo={getEffectiveKeybinding(keybindings, hotkey.id)}
              isCustom={!!keybindings[hotkey.id]}
              clashes={clashesWith(hotkey.id)}
              capturing={capturing === hotkey.id}
              onStartCapture={() => setCapturing(hotkey.id)}
              onCapture={async (combo) => {
                setCapturing(null)
                await Promise.resolve(updateKeybinding(hotkey.id, combo))
                return useSettingsStore.getState().keybindings[hotkey.id] === combo
              }}
              onCancelCapture={() => setCapturing(null)}
              onReset={async () => {
                await Promise.resolve(resetKeybinding(hotkey.id))
                return !useSettingsStore.getState().keybindings[hotkey.id]
              }}
            />
          ))}
        </SettingsGroup>
      ))}

      <SettingsGroup card title="Reset" testId="keys-reset">
        <SettingRow
          settingId="keybindings.reset-all"
          label="Reset all shortcuts"
          hint={
            resetNote ? (
              <span role="status" data-testid="keys-reset-note">
                {resetNote}
              </span>
            ) : changed > 0 ? (
              `Puts the ${changed === 1 ? 'one shortcut' : `${changed} shortcuts`} you changed back to K2’s defaults.`
            ) : (
              'Every shortcut is on its default.'
            )
          }
        >
          <Button variant="danger-quiet" disabled={changed === 0} onClick={() => setConfirmReset(true)}>
            Reset all…
          </Button>
        </SettingRow>
      </SettingsGroup>

      {confirmReset && (
        <ResetAllDialog
          changed={changed}
          onClose={() => setConfirmReset(false)}
          onDone={() => {
            setConfirmReset(false)
            setResetNote('All shortcuts are back to their defaults.')
          }}
        />
      )}
    </div>
  )
}

function ResetAllDialog({ changed, onClose, onDone }: { changed: number; onClose: () => void; onDone: () => void }): React.JSX.Element {
  const resetAllKeybindings = useSettingsStore((s) => s.resetAllKeybindings)
  const [busy, setBusy] = useState(false)
  const [failed, setFailed] = useState(false)
  const run = async (): Promise<void> => {
    setBusy(true)
    setFailed(false)
    await Promise.resolve(resetAllKeybindings())
    setBusy(false)
    if (Object.keys(useSettingsStore.getState().keybindings).length === 0) onDone()
    else setFailed(true)
  }
  return (
    <AccessSheet
      title="Reset all shortcuts?"
      intro={`The ${changed === 1 ? 'shortcut' : `${changed} shortcuts`} you changed go back to K2’s defaults. You can change them again any time.`}
      onClose={onClose}
      busy={busy}
      testId="keys-reset-confirm"
    >
      {failed && (
        <FailedNote
          what="Your shortcuts weren’t reset."
          next="They are as they were. Try again in a moment."
          detail="the settings didn’t save"
        />
      )}
      <div className="flex justify-end gap-2 pt-1">
        <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
          Cancel
        </Button>
        <Button variant="danger-muted" size="md" onClick={() => void run()} disabled={busy}>
          {busy ? 'Resetting…' : 'Reset all'}
        </Button>
      </div>
    </AccessSheet>
  )
}

interface KeybindingRowProps {
  hotkey: HotkeyDefinition
  combo: string
  isCustom: boolean
  /** Labels of the other shortcuts on the same keys. */
  clashes: string[]
  capturing: boolean
  onStartCapture: () => void
  /** Saves the new keys; true when they stuck. */
  onCapture: (combo: string) => Promise<boolean>
  onCancelCapture: () => void
  /** Back to the default; true when it stuck. */
  onReset: () => Promise<boolean>
}

function KeybindingRow({
  hotkey,
  combo,
  isCustom,
  clashes,
  capturing,
  onStartCapture,
  onCapture,
  onCancelCapture,
  onReset,
}: KeybindingRowProps): React.JSX.Element {
  const captureRef = useRef<HTMLButtonElement>(null)
  const save = useSaveStatus()
  /** Keys the last press tried that can't be a shortcut, and why. */
  const [refused, setRefused] = useState<string | null>(null)

  useEffect(() => {
    if (!capturing) return

    const handler = (e: KeyboardEvent): void => {
      e.preventDefault()
      e.stopPropagation()

      if (e.key === 'Escape') {
        setRefused(null)
        onCancelCapture()
        return
      }

      // Ignore bare modifier presses
      if (['Meta', 'Control', 'Alt', 'Shift'].includes(e.key)) return
      const newCombo = keyEventToCombo(e)

      if (isReservedKey(newCombo)) {
        setRefused(`${formatKeyCombo(newCombo, isMacPlatform())} goes to the program in the terminal, so it can’t be a shortcut. Try other keys.`)
        return
      }
      if (isZenChordCombo(newCombo)) {
        setRefused(`${formatKeyCombo(newCombo, isMacPlatform())} enters and leaves Zen mode. Try other keys.`)
        return
      }

      setRefused(null)
      save.reset()
      void onCapture(newCombo).then((ok) => (ok ? save.saved() : save.failed('')))
    }

    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [capturing, onCapture, onCancelCapture])

  useEffect(() => {
    if (capturing && captureRef.current) {
      captureRef.current.focus()
    }
  }, [capturing])

  const conflict = clashes.length > 0
  const hint = capturing ? (
    refused ? (
      <span role="alert" className="text-[var(--color-status-error-soft)]">
        {refused}
      </span>
    ) : (
      'Press the new keys. Escape cancels.'
    )
  ) : conflict ? (
    <span className="text-[var(--color-status-error-soft)]" data-keybinding-conflict="">
      Same keys as {clashes.join(' and ')}. Change one of them.
    </span>
  ) : isCustom ? (
    <>
      Changed. The default is {keysFor(hotkey.defaultKey)}.
    </>
  ) : undefined

  return (
    <SettingRow
      settingId={`keybindings.${hotkey.id}`}
      testId={`keybinding-${hotkey.id}`}
      label={hotkey.label}
      hint={
        hint || save.state !== 'idle' ? (
          <>
            {hint} <SaveNote state={save.state} />
            <SaveError state={save.state} />
          </>
        ) : undefined
      }
    >
      {isCustom && !capturing && (
        <Button
          variant="quiet"
          title="Reset to default"
          aria-label={`Reset ${hotkey.label} to default`}
          onClick={() => {
            save.reset()
            void onReset().then((ok) => (ok ? save.saved() : save.failed('')))
          }}
        >
          Reset
        </Button>
      )}
      {capturing ? (
        <button
          ref={captureRef}
          type="button"
          data-k2-chamfer="chip"
          aria-label={`Recording a new shortcut for ${hotkey.label}`}
          onClick={() => {
            setRefused(null)
            onCancelCapture()
          }}
          className={cx(KEY_CHIP, 'k2-focus no-drag cursor-pointer bg-[var(--color-bg)] border-[var(--color-accent)] text-[var(--color-text-primary)]')}
        >
          Press keys…
        </button>
      ) : (
        <button
          type="button"
          data-k2-chamfer="chip"
          aria-label={`${hotkey.label}: ${formatKeyCombo(combo, isMacPlatform())}. Change`}
          onClick={() => {
            setRefused(null)
            save.reset()
            onStartCapture()
          }}
          className={cx(
            KEY_CHIP,
            'k2-focus no-drag cursor-pointer transition-colors',
            conflict
              ? 'bg-[var(--color-bg-elevated)] border-[var(--color-status-error-soft)] text-[var(--color-status-error-soft)]'
              : cx(KEY_CHIP_REST, 'hover:border-[var(--color-text-muted)]'),
          )}
        >
          <KeyCombo combo={formatKeyCombo(combo, isMacPlatform())} />
        </button>
      )}
    </SettingRow>
  )
}
