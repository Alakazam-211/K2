import React from 'react'
import { useSettingsStore } from '@/stores/settings'
import {
  useTerminalSettingsStore,
  LINE_HEIGHT_MULT_DEFAULT,
  LINE_HEIGHT_MULT_MIN,
  LINE_HEIGHT_MULT_MAX,
  CHAR_TRACKING_DEFAULT,
  CHAR_TRACKING_MIN,
  CHAR_TRACKING_MAX,
} from '@/stores/terminal-settings'
import type { LinkClickMode } from '@/stores/terminal-settings'
import { TERMINAL_FONT_SIZE_MAX, TERMINAL_FONT_SIZE_MIN } from '@shared/constants'
import { EDITOR_FONTS } from '@/components/FileViewerPane/CodeEditor'
import { TERMINAL_DEFAULT_FONT } from '@/kessel/config'
import {
  SaveError,
  SaveNote,
  SETTING_ROW_HINT,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  useSaveStatus,
} from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { Button, Input, Slider, Toggle } from '@/components/ui'

export const TERMINAL_MANIFEST: SettingEntry[] = [
  { id: 'terminal.font-family', section: 'terminal', group: 'Text', label: 'Font', description: 'The font for terminal text', keywords: ['font', 'typeface', 'family'] },
  { id: 'terminal.font-size', section: 'terminal', group: 'Text', label: 'Size', description: 'Terminal text size on this computer; ⌘+ and ⌘− change it too', keywords: ['font', 'size', 'zoom', 'text size'] },
  { id: 'terminal.line-height', section: 'terminal', group: 'Text', label: 'Line spacing', description: 'Space between lines, as a multiple of the text size', keywords: ['line height', 'leading', 'spacing', 'rows', 'dense', 'open', 'webgl'] },
  { id: 'terminal.char-tracking', section: 'terminal', group: 'Text', label: 'Letter spacing', description: 'Space between letters', keywords: ['tracking', 'letter spacing', 'character spacing', 'kerning', 'width', 'cell', 'open', 'tight', 'webgl'] },
  { id: 'terminal.painter', section: 'terminal', group: 'Text', label: 'Draw with the GPU', description: 'Faster drawing; turn off if terminal text looks wrong', keywords: ['painter', 'renderer', 'kessel', 'webgl', 'gpu', 'canvas', 'rendering', 'dom'] },
  { id: 'terminal.natural-text-editing', section: 'terminal', group: 'Behaviour', label: 'Mac text shortcuts', description: '⌥ arrows move by word, ⌘ arrows to the line start or end', keywords: ['natural text editing', 'keyboard', 'edit', 'opt', 'alt', 'word'] },
  { id: 'terminal.link-click-mode', section: 'terminal', group: 'Behaviour', label: 'Open links with', description: 'A click or ⌘-click opens links in terminal output', keywords: ['link', 'url', 'click', 'cmd'] },
  { id: 'terminal.open-links-in-split', section: 'terminal', group: 'Behaviour', label: 'Open file links beside the terminal', description: 'In a split tab, file links open in the other pane', keywords: ['link', 'split', 'pane'] },
]

type Status = ReturnType<typeof useSaveStatus>

/** A slider with its number field and unit, on the row's right edge. */
function SliderField({
  label,
  value,
  min,
  max,
  step,
  digits = 0,
  unit,
  disabled = false,
  onChange,
}: {
  label: string
  value: number
  min: number
  max: number
  step: number
  digits?: number
  unit?: string
  disabled?: boolean
  onChange: (v: number) => void
}): React.JSX.Element {
  return (
    <div className="flex items-center gap-3">
      <Slider
        aria-label={label}
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(parseFloat(e.target.value))}
        className="w-36"
      />
      <Input
        type="number"
        aria-label={`${label}, exact value`}
        min={min}
        max={max}
        step={step}
        value={Number(value.toFixed(digits))}
        disabled={disabled}
        onChange={(e) => {
          const v = parseFloat(e.target.value)
          if (Number.isFinite(v) && v >= min && v <= max) onChange(v)
        }}
        className="w-16 text-center type-control disabled:opacity-50"
      />
      {unit && <span className="type-hint text-[var(--color-text-secondary)] w-4">{unit}</span>}
    </div>
  )
}

/** The row's hint with its save note and any save error under it. */
function Hint({ status, children }: { status: Status; children: React.ReactNode }): React.JSX.Element {
  return (
    <>
      {children} <SaveNote state={status.state} />
      <SaveError state={status.state} />
    </>
  )
}

export function TerminalSection(): React.JSX.Element {
  const terminal = useSettingsStore((s) => s.terminal)
  const updateTerminalSettings = useSettingsStore((s) => s.updateTerminalSettings)
  const fontSize = useTerminalSettingsStore((s) => s.fontSize)
  const setFontSize = useTerminalSettingsStore((s) => s.setFontSize)
  const linkClickMode = useTerminalSettingsStore((s) => s.linkClickMode)
  const setLinkClickMode = useTerminalSettingsStore((s) => s.setLinkClickMode)
  const openLinksInSplitPane = useTerminalSettingsStore((s) => s.openLinksInSplitPane)
  const setOpenLinksInSplitPane = useTerminalSettingsStore((s) => s.setOpenLinksInSplitPane)
  const painter = useTerminalSettingsStore((s) => s.painter)
  const setPainter = useTerminalSettingsStore((s) => s.setPainter)
  const lineHeightMultiplier = useTerminalSettingsStore((s) => s.lineHeightMultiplier)
  const setLineHeightMultiplier = useTerminalSettingsStore((s) => s.setLineHeightMultiplier)
  const charTracking = useTerminalSettingsStore((s) => s.charTracking)
  const setCharTracking = useTerminalSettingsStore((s) => s.setCharTracking)

  const fontSave = useSaveStatus()
  const sizeSave = useSaveStatus()
  const lineSave = useSaveStatus()
  const letterSave = useSaveStatus()
  const gpuSave = useSaveStatus()
  const naturalSave = useSaveStatus()
  const linkSave = useSaveStatus()
  const splitSave = useSaveStatus()

  const gpu = painter === 'webgl'
  const fontFamily = terminal.fontFamily || TERMINAL_DEFAULT_FONT
  const fontOptions = EDITOR_FONTS.map((f) => ({ value: f.id, label: f.label }))
  if (!fontOptions.some((o) => o.value === fontFamily)) fontOptions.push({ value: fontFamily, label: fontFamily })
  const spacingAtDefault = lineHeightMultiplier === LINE_HEIGHT_MULT_DEFAULT && charTracking === CHAR_TRACKING_DEFAULT

  /** A write to the server: Saved when the store kept it, or why not. */
  const saveServer = async (partial: Partial<typeof terminal>, status: Status): Promise<void> => {
    status.reset()
    await Promise.resolve(updateTerminalSettings(partial))
    const now = useSettingsStore.getState().terminal
    const kept = (Object.keys(partial) as (keyof typeof terminal)[]).every((k) => now?.[k] === partial[k])
    if (kept) status.saved()
    else status.failed('')
  }
  /** A setting kept on this computer: it applies at once. */
  const saveLocal = (apply: () => void, status: Status): void => {
    apply()
    status.saved()
  }

  const gpuOnly = gpu ? null : <span className="block">Needs Draw with the GPU, below.</span>

  return (
    <div className="w-full max-w-[52rem] space-y-6 pb-6" data-testid="terminal-page">
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)]">Terminal</h2>
        <p className={SETTING_ROW_HINT}>How text looks and behaves in terminal tabs.</p>
      </header>

      <SettingsGroup
        card
        title="Text"
        action={
          <Button
            variant="quiet"
            disabled={spacingAtDefault || !gpu}
            onClick={() => {
              setLineHeightMultiplier(LINE_HEIGHT_MULT_DEFAULT)
              setCharTracking(CHAR_TRACKING_DEFAULT)
              lineSave.saved()
              letterSave.saved()
            }}
          >
            Reset spacing
          </Button>
        }
        testId="terminal-text"
      >
        <SettingRow
          settingId="terminal.font-family"
          label="Font"
          hint={<Hint status={fontSave}>The default is {TERMINAL_DEFAULT_FONT}. If the font you pick isn’t installed on this Mac, terminals use it instead.</Hint>}
          field
        >
          <SettingDropdown
            fullWidth
            ariaLabel="Font"
            value={fontFamily}
            options={fontOptions}
            onChange={(v) => saveServer({ fontFamily: v }, fontSave)}
          />
        </SettingRow>
        <SettingRow
          settingId="terminal.font-size"
          label="Size"
          hint={<Hint status={sizeSave}>⌘+ and ⌘− change it too. On this computer only.</Hint>}
        >
          <SliderField
            label="Size"
            value={fontSize}
            min={TERMINAL_FONT_SIZE_MIN}
            max={TERMINAL_FONT_SIZE_MAX}
            step={1}
            unit="px"
            onChange={(v) => saveLocal(() => setFontSize(v), sizeSave)}
          />
        </SettingRow>
        <SettingRow
          settingId="terminal.line-height"
          label="Line spacing"
          hint={
            <Hint status={lineSave}>
              Space between lines, as a multiple of the text size. Default {LINE_HEIGHT_MULT_DEFAULT}.{gpuOnly}
            </Hint>
          }
        >
          <SliderField
            label="Line spacing"
            value={lineHeightMultiplier}
            min={LINE_HEIGHT_MULT_MIN}
            max={LINE_HEIGHT_MULT_MAX}
            step={0.02}
            digits={2}
            unit="×"
            disabled={!gpu}
            onChange={(v) => saveLocal(() => setLineHeightMultiplier(v), lineSave)}
          />
        </SettingRow>
        <SettingRow
          settingId="terminal.char-tracking"
          label="Letter spacing"
          hint={
            <Hint status={letterSave}>
              Space between letters. Default {CHAR_TRACKING_DEFAULT}.{gpuOnly}
            </Hint>
          }
        >
          <SliderField
            label="Letter spacing"
            value={charTracking}
            min={CHAR_TRACKING_MIN}
            max={CHAR_TRACKING_MAX}
            step={0.01}
            digits={2}
            unit="×"
            disabled={!gpu}
            onChange={(v) => saveLocal(() => setCharTracking(v), letterSave)}
          />
        </SettingRow>
        <SettingRow
          settingId="terminal.painter"
          label="Draw with the GPU"
          hint={
            <Hint status={gpuSave}>
              Faster, and needed for line and letter spacing. Turn it off if terminal text looks wrong. Applies to
              terminals you open next.
            </Hint>
          }
        >
          <Toggle
            size="sm"
            checked={gpu}
            aria-label="Draw with the GPU"
            onChange={() => saveLocal(() => setPainter(gpu ? 'dom' : 'webgl'), gpuSave)}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup card title="Behaviour" testId="terminal-behaviour">
        <SettingRow
          settingId="terminal.natural-text-editing"
          label="Mac text shortcuts"
          hint={
            <Hint status={naturalSave}>
              ⌥← and ⌥→ move by word, ⌘← and ⌘→ go to the start or end of the line, and ⌥⌫ deletes a word.
            </Hint>
          }
        >
          <Toggle
            size="sm"
            checked={terminal.naturalTextEditing !== false}
            aria-label="Mac text shortcuts"
            onChange={() => void saveServer({ naturalTextEditing: terminal.naturalTextEditing === false }, naturalSave)}
          />
        </SettingRow>
        <SettingRow
          settingId="terminal.link-click-mode"
          label="Open links with"
          hint={<Hint status={linkSave}>For web links and file paths in terminal output.</Hint>}
          field
        >
          <SettingDropdown
            fullWidth
            ariaLabel="Open links with"
            value={linkClickMode}
            options={[
              { value: 'click', label: 'A click' },
              { value: 'cmd-click', label: '⌘-click' },
            ]}
            onChange={(v) => saveLocal(() => setLinkClickMode(v as LinkClickMode), linkSave)}
          />
        </SettingRow>
        <SettingRow
          settingId="terminal.open-links-in-split"
          label="Open file links beside the terminal"
          hint={<Hint status={splitSave}>In a split tab, a file link opens in the other pane instead of a new tab.</Hint>}
        >
          <Toggle
            size="sm"
            checked={openLinksInSplitPane}
            aria-label="Open file links beside the terminal"
            onChange={() => saveLocal(() => setOpenLinksInSplitPane(!openLinksInSplitPane), splitSave)}
          />
        </SettingRow>
      </SettingsGroup>
    </div>
  )
}
