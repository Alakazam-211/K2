// The "Sample data" banner, for every live view while a sample drives it:
// it says this is sample data, not the file the person opened, and holds
// the sample's player (play, pause, step, start over) and its scenarios.
// The workbook's banner (../SampleBanner.tsx) is a thin adapter over this.

import { Button, Segmented } from '@/components/ui'
import { InfoGlyph, PauseGlyph, PlayGlyph, RestartGlyph, StepGlyph } from '../parts'

export const SAMPLE_BANNER_TITLE = 'Sample data, not this file'

export interface SampleBannerStatus {
  playing: boolean
  position: number
  length: number
  label?: string | null
}

export function SampleBannerView<T extends string>({
  line,
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
  scenarios: readonly { value: T; label: string }[]
  scenario: T
  onScenario: (s: T) => void
  status: SampleBannerStatus
  onPlay: () => void
  onPause: () => void
  onStep: () => void
  onRestart: () => void
}): React.JSX.Element {
  return (
    <div className="flex flex-none flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-[color-mix(in_srgb,var(--color-status-warn)_35%,var(--color-border))] bg-[color-mix(in_srgb,var(--color-status-warn)_8%,var(--color-bg))] px-3 py-1.5" role="note" aria-label={SAMPLE_BANNER_TITLE} data-testid="lv-sample-banner">
      <div className="flex min-w-[260px] flex-1 items-start gap-2">
      <span aria-hidden="true" className="mt-0.5 inline-flex flex-none text-[var(--color-status-warn-text)]">
        <InfoGlyph size={15} />
      </span>
      <div className="min-w-0 flex-1">
        <span className="type-hint font-semibold text-[var(--color-text-primary)]">{SAMPLE_BANNER_TITLE}.</span> <span className="type-hint text-[var(--color-text-secondary)]">{line}</span>
      </div>
      </div>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
      <Segmented<T> aria-label="Sample" value={scenario} onChange={onScenario} options={scenarios.map((s) => ({ value: s.value, label: s.label, buttonProps: { 'data-testid': `lv-sample-scenario-${s.value}` } }))} />
      <div className="flex items-center gap-1" role="group" aria-label="Sample player">
        <Button size="sm" variant="ghost" aria-label={status.playing ? 'Pause the sample' : 'Play the sample'} onClick={status.playing ? onPause : onPlay} data-testid="lv-sample-play">
          {status.playing ? <PauseGlyph size={12} /> : <PlayGlyph size={12} />}
        </Button>
        <Button size="sm" variant="ghost" aria-label="Next step" onClick={onStep} data-testid="lv-sample-step">
          <StepGlyph size={12} />
        </Button>
        <Button size="sm" variant="ghost" aria-label="Start over" onClick={onRestart} data-testid="lv-sample-restart">
          <RestartGlyph size={12} />
        </Button>
        <span className="lv-num type-hint text-[var(--color-text-secondary)]" aria-live="off" data-testid="lv-sample-position">
          {Math.min(status.position, status.length)}/{status.length}
          {status.label ? ` · ${status.label}` : ''}
        </span>
      </div>
      </div>
    </div>
  )
}
