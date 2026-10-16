// The machine chip on a Home row's second line (which server it is on),
// with that server's reach on its icon's LEDs.

import { REACH_COPY, REACH_LED, type RowReach } from '@/lib/home-agent-status'
import { ServerIcon } from '@/components/icons/K2Icons'

/** Which machine a row is on — sized like the Agents row's group chips.
 *  With `reach`, its server icon's LEDs say whether that server is there
 *  (the top-bar server switcher's language, on tokens; plan-agent-status-
 *  unify-v1 §3.1 "Reach"). */
export function MachineChip({ place, reach }: { place: string; reach?: RowReach }): React.JSX.Element {
  return (
    <span
      className="box-border inline-flex h-3.5 max-w-[6.5rem] items-center gap-[3px] truncate px-1 py-0 text-[9px] font-medium leading-none border border-[var(--color-border-strong)] text-[var(--color-text-muted)]"
      title={reach ? `On ${place} · ${REACH_COPY[reach]}` : `On ${place}`}
      data-machine-chip=""
      data-reach={reach}
    >
      <ServerIcon size={9} ledColor={reach ? REACH_LED[reach] : undefined} />
      <span className="truncate">{place}</span>
    </span>
  )
}
