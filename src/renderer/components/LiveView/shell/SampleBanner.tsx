// The "Sample data" banner, for every live view while a sample drives it:
// it says this is sample data, not the file the person opened, and holds
// the sample's player (play, pause, step, start over) and its scenarios.
// The workbook's banner (../SampleBanner.tsx) is a thin adapter over this.

import { Button, Segmented } from '@/components/ui'
import { SettingDropdown } from '@/components/Settings/controls/SettingControls'
import { InfoGlyph, PauseGlyph, PlayGlyph, RestartGlyph, StepGlyph } from '../parts'

export const SAMPLE_BANNER_TITLE = 'Sample data, not this file'
/** More scenarios than this (the scene's 24) go in a grouped menu instead of segments. */
const MAX_SEGMENTS = 5

export interface SampleBannerStatus {
  playing: boolean
  position: number
  length: number
  label?: string | null
}

export function SampleBannerView<T extends string>({
  line,
  title = SAMPLE_BANNER_TITLE,
  scenarios,
  scenario,
  onScenario,
  status,
  onPlay,
  onPause,
  onStep,
  onRestart,
}: {
  line: string
  /** The bold words before the line (default: "Sample data, not this file"). */
  title?: string
  scenarios: readonly { value: T; label: string; group?: string }[]
  scenario: T
  onScenario: (s: T) => void
  status: SampleBannerStatus
  onPlay: () => void
  onPause: () => void
  onStep: () => void
  onRestart: () => void
}): React.JSX.Element {
  return (
    <div className="flex flex-none items-center gap-3 border-b border-[color-mix(in_srgb,var(--color-status-warn)_35%,var(--color-border))] bg-[color-mix(in_srgb,var(--color-status-warn)_8%,var(--color-bg))] px-3 py-1" role="note" aria-label={title} data-testid="lv-sample-banner" data-one-row="">
      <div className="flex min-w-[112px] flex-1 items-center gap-2" title={`${title}. ${line}`}>
      <span aria-hidden="true" className="inline-flex flex-none text-[var(--color-status-warn-text)]">
        <InfoGlyph size={15} />
      </span>
      {/* One row, as the mock's chip and banner: the line gives way (its whole text is the tooltip). */}
      <div className="min-w-0 flex-1 truncate whitespace-nowrap">
        <span className="type-hint font-semibold text-[var(--color-text-primary)]">{title}.</span> <span className="type-hint text-[var(--color-text-secondary)]">{line}</span>
      </div>
      </div>
      <div className="flex min-w-0 items-center gap-2">
      <span data-testid="lv-sample-scenarios" className="inline-flex min-w-0 shrink">
        {scenarios.length > MAX_SEGMENTS ? (
          <SettingDropdown
            compact
            ariaLabel="Sample"
            value={scenario}
            onChange={(v) => onScenario(v as T)}
            options={scenarios.map((s) => ({ value: s.value, label: s.label, ...(s.group ? { group: s.group } : {}) }))}
            className="min-w-0 max-w-[300px]"
          />
        ) : (
          <Segmented<T> aria-label="Sample" value={scenario} onChange={onScenario} options={scenarios.map((s) => ({ value: s.value, label: s.label, buttonProps: { 'data-testid': `lv-sample-scenario-${s.value}` } }))} />
        )}
      </span>
      <div className="flex min-w-0 items-center gap-1" role="group" aria-label="Sample player">
        <Button size="sm" variant="ghost" aria-label={status.playing ? 'Pause the sample' : 'Play the sample'} onClick={status.playing ? onPause : onPlay} data-testid="lv-sample-play">
          {status.playing ? <PauseGlyph size={12} /> : <PlayGlyph size={12} />}
        </Button>
        <Button size="sm" variant="ghost" aria-label="Next step" onClick={onStep} data-testid="lv-sample-step">
          <StepGlyph size={12} />
        </Button>
        <Button size="sm" variant="ghost" aria-label="Start over" onClick={onRestart} data-testid="lv-sample-restart">
          <RestartGlyph size={12} />
        </Button>
        <span className="lv-num type-hint min-w-0 truncate whitespace-nowrap text-[var(--color-text-secondary)]" aria-live="off" data-testid="lv-sample-position">
          {Math.min(status.position, status.length)}/{status.length}
          {status.label ? ` · ${status.label}` : ''}
        </span>
      </div>
      </div>
    </div>
  )
}
