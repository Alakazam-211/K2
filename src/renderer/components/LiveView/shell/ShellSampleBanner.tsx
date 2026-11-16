// The "Sample data" banner over a sample shell source's player: its
// scenarios, play, pause, step and start over.

import { useEffect, useState } from 'react'
import type { PlayerStatus, SamplePlayer } from '@/lib/live-view/shared/sample-player'
import { SampleBannerView } from './SampleBanner'

export function ShellSampleBanner<S>({ player, line }: { player: SamplePlayer<S>; line: string }): React.JSX.Element {
  const [st, setSt] = useState<PlayerStatus>(() => player.status())
  useEffect(() => {
    setSt(player.status())
    return player.onStatus(setSt)
  }, [player])
  return (
    <SampleBannerView<string>
      line={line}
      scenarios={player.scenarioList().map((s) => ({ value: s.id, label: s.label }))}
      scenario={st.scenario}
      onScenario={(id) => player.setScenario(id)}
      status={st}
      onPlay={() => player.play()}
      onPause={() => player.pause()}
      onStep={() => player.step()}
      onRestart={() => player.restart()}
    />
  )
}
