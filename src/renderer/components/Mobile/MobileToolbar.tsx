// The Mobile tab's top bar (prd-mobile-dev-v1 §2.2; slimmed by
// research-mobile-pane-polish-v1 §4 item 2):
//   [platform ▾] [device ▾] ………… [Take over | Hand back]  /  [Watching]
// The device's own controls live on the rail beside the phone
// (MobileControlRail); Start is on the empty state, Stop in the rail's ⋯.
// Every control is a gesture to the daemon; the bar keeps no device state.
// A watch-only viewer (remote Connect Members, Rosson 2026-10-27) sees the
// device and "Watching" only.

import { SettingDropdown } from '@/components/Settings/controls/SettingControls'
import { THEME_PILL_RADIUS } from './MobileControlRail'
import type { SimDeviceProfile, SimLaneId, SimPlatform, SimView } from './simApi'

const PLATFORM_NAME: Record<SimPlatform, string> = { android: 'Android', ios: 'iPhone' }

export interface MobileToolbarProps {
  session: SimView | null
  /** The lane the chooser's devices boot on (before any device runs). */
  lane: SimLaneId | null
  /** The platforms this server can run (from the daemon's usable lanes);
   *  a chooser shows only when there are two. */
  platforms?: readonly SimPlatform[]
  platform?: SimPlatform | null
  onPlatform?(platform: SimPlatform): void
  devices: SimDeviceProfile[]
  /** The chooser's value (the running device, else this tab's last pick). */
  device: string
  canControl: boolean
  /** A boot / stop / screenshot request is in flight. */
  busy: boolean
  /** The stream is open and drawing (Take over needs it). */
  live: boolean
  /** This viewer holds the device with Take over. */
  tookOver: boolean
  onDevice(device: string): void
  onTakeOver(): void
  onHandBack(): void
}

export function MobileToolbar(props: MobileToolbarProps): React.JSX.Element {
  const { session, devices, device, canControl, busy, live, tookOver } = props
  const running = session?.state === 'running'
  const changing = session?.state === 'installing' || session?.state === 'booting' || session?.state === 'stopping'
  const on = running || changing
  const profile = session?.profile
  const choices: Array<{ id: string; name: string }> =
    devices.length > 0 ? devices : profile ? [{ id: profile.id, name: profile.name }] : []
  return (
    <div
      className="flex h-10 flex-shrink-0 items-center gap-1.5 border-b border-[var(--color-border)] px-2 text-xs"
      data-testid="mobile-toolbar"
    >
      {props.platforms && props.platforms.length > 1 ? (
        <SettingDropdown
          ariaLabel="Platform"
          compact
          menuAlign="left"
          className="flex-shrink-0"
          value={props.platform ?? props.platforms[0]}
          options={props.platforms.map((p) => ({ value: p, label: PLATFORM_NAME[p] }))}
          disabled={!canControl || busy || on}
          onChange={(v) => props.onPlatform?.(v as SimPlatform)}
          triggerData={{ 'data-mobile-platform': '' }}
        />
      ) : null}
      <SettingDropdown
        ariaLabel="Device"
        compact
        menuAlign="left"
        className="min-w-0 max-w-[12rem]"
        value={device}
        options={choices.map((d) => ({ value: d.id, label: d.name }))}
        placeholder={choices.length === 0 ? 'No devices' : undefined}
        disabled={!canControl || busy || changing || choices.length === 0}
        onChange={(v) => props.onDevice(v)}
        triggerData={{ 'data-mobile-device': '' }}
      />
      <span className="flex-1" />
      {canControl && running ? (
        <button
          type="button"
          className={`h-6 flex-shrink-0 cursor-pointer border px-3 text-xs transition-colors disabled:cursor-default disabled:opacity-50 ${
            tookOver
              ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/10 text-[var(--color-accent)]'
              : 'border-[var(--color-border)] text-[var(--color-text-secondary)] enabled:hover:text-[var(--color-text-primary)]'
          }`}
          style={{ borderRadius: THEME_PILL_RADIUS }}
          onClick={tookOver ? props.onHandBack : props.onTakeOver}
          disabled={!live}
          title={
            tookOver
              ? 'Give the device back to the agent'
              : 'Hold the device: the agent waits until you hand it back (or 10 minutes with no input)'
          }
          data-mobile-tool={tookOver ? 'hand-back' : 'take-over'}
        >
          {tookOver ? 'Hand back' : 'Take over'}
        </button>
      ) : null}
      {!canControl ? (
        <span
          className="flex-shrink-0 border border-[var(--color-border)] px-2.5 py-0.5 text-[var(--color-text-muted)]"
          style={{ borderRadius: THEME_PILL_RADIUS }}
          data-mobile-watching=""
          title="You can watch this device; your login can't control it"
        >
          Watching
        </span>
      ) : null}
    </div>
  )
}
