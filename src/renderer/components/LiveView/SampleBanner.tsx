// Always on while the sample drives the view: it says this is sample data,
// not the file the person opened, and holds the sample's player (play, pause,
// step, start over) and its scenarios.

import { useEffect, useState } from 'react'
import { Button, Segmented } from '@/components/ui'
import type { SampleScenario, SampleStatus, SampleWorkbookSource } from '@/lib/live-view/sample-source'
import { InfoGlyph, PauseGlyph, PlayGlyph, RestartGlyph, StepGlyph } from './parts'

export const SAMPLE_BANNER_TITLE = 'Sample data, not this file'
export const SAMPLE_BANNER_LINE = 'A preview of watching a workbook live: an agent’s turn played from K2’s sample workbook. Nothing here comes from the file you opened.'

export function SampleBanner({ sample, scenario, onScenario }: { sample: SampleWorkbookSource; scenario: SampleScenario; onScenario: (s: SampleScenario) => void }): React.JSX.Element {
  const [st, setSt] = useState<SampleStatus>(() => sample.status())
  useEffect(() => {
    setSt(sample.status())
    return sample.onStatus(setSt)
  }, [sample])
  return (
    <div className="flex flex-wrap items-center gap-2 border-b border-[var(--color-status-warn-soft)] bg-[var(--color-status-warn-soft)] px-3 py-1.5" role="note" aria-label={SAMPLE_BANNER_TITLE} data-testid="lv-sample-banner">
      <span className="text-[var(--color-status-warn-text)]">
        <InfoGlyph size={15} />
      </span>
      <div className="min-w-0 flex-1">
        <span className="type-row font-semibold text-[var(--color-text-primary)]">{SAMPLE_BANNER_TITLE}.</span>{' '}
        <span className="type-hint text-[var(--color-text-secondary)]">{SAMPLE_BANNER_LINE}</span>
      </div>
      <Segmented<SampleScenario>
        aria-label="Sample"
        value={scenario}
        onChange={onScenario}
        options={[
          { value: 'turn', label: 'Live turn' },
          { value: 'blob', label: 'Too large' },
          { value: 'none', label: 'Not opened' },
        ]}
      />
      <div className="flex items-center gap-1" role="group" aria-label="Sample player">
        <Button size="sm" variant="ghost" aria-label={st.playing ? 'Pause the sample' : 'Play the sample'} onClick={() => (st.playing ? sample.pause() : sample.play())} data-testid="lv-sample-play">
          {st.playing ? <PauseGlyph size={12} /> : <PlayGlyph size={12} />}
        </Button>
        <Button size="sm" variant="ghost" aria-label="Next step" onClick={() => sample.step()} data-testid="lv-sample-step">
          <StepGlyph size={12} />
        </Button>
        <Button size="sm" variant="ghost" aria-label="Start over" onClick={() => sample.restart()} data-testid="lv-sample-restart">
          <RestartGlyph size={12} />
        </Button>
        <span className="lv-num type-hint text-[var(--color-text-muted)]" aria-live="off">
          {Math.min(st.position, st.length)}/{st.length}
          {st.label ? ` · ${st.label}` : ''}
        </span>
      </div>
    </div>
  )
}
