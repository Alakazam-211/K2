// The Mobile tab's device toolbar (prd-mobile-dev-v1 §2.2):
//   [device ▾] [Boot|Stop] · [Rotate] [Home] [Back] [Screenshot] · [Take over]
// Back is Android only. Every control is a gesture to the daemon; the tab
// keeps no device state of its own. A watch-only viewer (remote Connect
// Members, Rosson 2026-10-27) sees the device name and "Watching" only.
//
// Not in this slice: the ⋯ menu extras (the PRD's first cut, §9.3) and
// Select (S5 pick → note → Send, undecided). `extra` is the slot S5's
// Select toggle goes in.

import {
  ArrowLeftIcon,
  CameraIcon,
  HomeIcon,
  K2_ICON_PX,
  PowerIcon,
  RotateIcon,
} from '@/components/icons/K2Icons'
import type { SimDeviceProfile, SimLaneId, SimPlatform, SimView } from './simApi'

const PLATFORM_NAME: Record<SimPlatform, string> = { android: 'Android', ios: 'iPhone' }
import { REMOTE_FPS_CHOICES } from './sim-stream'

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
  /** A lane can boot now (not waiting on the owner's licence or step). */
  canBoot: boolean
  /** The stream is open and drawing (rotate / Home / Back need it). */
  live: boolean
  /** This viewer holds the device with Take over. */
  tookOver: boolean
  /** Remote viewers pick their frame rate (M-A19); null = local. */
  fps: number | null
  onDevice(device: string): void
  onBoot(): void
  onStop(): void
  onRotate(): void
  onHome(): void
  onBack(): void
  onScreenshot(): void
  onTakeOver(): void
  onHandBack(): void
  onFps(fps: number): void
  /** S5: the Select toggle. */
  extra?: React.ReactNode
}

function ToolButton({
  label,
  title,
  onClick,
  disabled,
  pressed,
  children,
  testId,
}: {
  label: string
  title?: string
  onClick(): void
  disabled?: boolean
  pressed?: boolean
  children: React.ReactNode
  testId: string
}): React.JSX.Element {
  return (
    <button
      type="button"
      aria-label={label}
      title={title ?? label}
      disabled={disabled}
      onClick={onClick}
      {...(pressed !== undefined ? { 'aria-pressed': pressed } : {})}
      data-mobile-tool={testId}
      className={`flex h-6 w-6 flex-shrink-0 items-center justify-center cursor-pointer disabled:cursor-default disabled:opacity-30 ${
        pressed ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text)]'
      }`}
    >
      {children}
    </button>
  )
}

const Sep = (): React.JSX.Element => <span className="mx-1 h-4 w-px flex-shrink-0 bg-[var(--color-border)]" aria-hidden />

export function MobileToolbar(props: MobileToolbarProps): React.JSX.Element {
  const { session, devices, device, canControl, busy, live, tookOver, fps } = props
  const running = session?.state === 'running'
  const changing = session?.state === 'installing' || session?.state === 'booting' || session?.state === 'stopping'
  const on = running || changing
  const lane = session?.lane ?? props.lane
  const profile = session?.profile
  const choices: Array<{ id: string; name: string }> =
    devices.length > 0 ? devices : profile ? [{ id: profile.id, name: profile.name }] : []
  return (
    <div
      className="flex h-9 flex-shrink-0 items-center gap-1 border-b border-[var(--color-border)] px-2 text-xs"
      data-testid="mobile-toolbar"
    >
      {props.platforms && props.platforms.length > 1 ? (
        <select
          aria-label="Platform"
          className="h-6 cursor-pointer rounded-none border border-[var(--color-border)] bg-[var(--color-bg)] px-1 text-xs text-[var(--color-text)] disabled:cursor-default disabled:opacity-60"
          value={props.platform ?? props.platforms[0]}
          disabled={!canControl || busy || on}
          onChange={(e) => props.onPlatform?.(e.target.value as SimPlatform)}
          data-mobile-platform=""
        >
          {props.platforms.map((p) => (
            <option key={p} value={p}>
              {PLATFORM_NAME[p]}
            </option>
          ))}
        </select>
      ) : null}
      <select
        aria-label="Device"
        className="h-6 min-w-0 max-w-[12rem] cursor-pointer rounded-none border border-[var(--color-border)] bg-[var(--color-bg)] px-1 text-xs text-[var(--color-text)] disabled:cursor-default disabled:opacity-60"
        value={device}
        disabled={!canControl || busy || changing || choices.length === 0}
        onChange={(e) => props.onDevice(e.target.value)}
        data-mobile-device=""
      >
        {choices.length === 0 ? <option value="">No devices</option> : null}
        {choices.map((d) => (
          <option key={d.id} value={d.id}>
            {d.name}
          </option>
        ))}
      </select>
      {canControl ? (
        <ToolButton
          label={on ? 'Stop the device' : 'Start the device'}
          onClick={on ? props.onStop : props.onBoot}
          disabled={busy || changing || (!on && (!device || !props.canBoot))}
          pressed={on}
          testId={on ? 'stop' : 'boot'}
        >
          <PowerIcon size={K2_ICON_PX} />
        </ToolButton>
      ) : null}
      {canControl ? (
        <>
          <Sep />
          <ToolButton label="Rotate" onClick={props.onRotate} disabled={!live} testId="rotate">
            <RotateIcon size={K2_ICON_PX} />
          </ToolButton>
          <ToolButton label="Home" title="Home (⌘⇧H on the device)" onClick={props.onHome} disabled={!live} testId="home">
            <HomeIcon size={K2_ICON_PX} />
          </ToolButton>
          {lane === 'android' ? (
            <ToolButton label="Back" onClick={props.onBack} disabled={!live} testId="back">
              <ArrowLeftIcon size={K2_ICON_PX} />
            </ToolButton>
          ) : null}
        </>
      ) : null}
      <ToolButton
        label="Screenshot"
        title="Save a screenshot to .k2/sim/shots/ and copy its path"
        onClick={props.onScreenshot}
        disabled={!running || busy || !canControl}
        testId="screenshot"
      >
        <CameraIcon size={K2_ICON_PX} />
      </ToolButton>
      {props.extra}
      {canControl && running ? (
        <>
          <Sep />
          <button
            type="button"
            className={`h-6 flex-shrink-0 cursor-pointer border px-2 text-xs ${
              tookOver
                ? 'border-[var(--color-accent)] text-[var(--color-accent)]'
                : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text)]'
            }`}
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
        </>
      ) : null}
      <span className="flex-1" />
      {fps !== null ? (
        <select
          aria-label="Frame rate"
          className="h-6 cursor-pointer rounded-none border border-[var(--color-border)] bg-[var(--color-bg)] px-1 text-xs text-[var(--color-text-secondary)]"
          value={fps}
          onChange={(e) => props.onFps(Number(e.target.value))}
          title="Frames per second over Connect"
          data-mobile-fps=""
        >
          {REMOTE_FPS_CHOICES.map((n) => (
            <option key={n} value={n}>
              {n} fps
            </option>
          ))}
        </select>
      ) : null}
      {!canControl ? (
        <span className="flex-shrink-0 text-[var(--color-text-muted)]" data-mobile-watching="" title="You can watch this device; your login can't control it">
          Watching
        </span>
      ) : null}
    </div>
  )
}
