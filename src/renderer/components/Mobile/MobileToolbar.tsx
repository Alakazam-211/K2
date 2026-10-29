// The Mobile tab's top bar (prd-mobile-dev-v1 §2.2; slimmed by
// research-mobile-pane-polish-v1 §4 items 2 and 4):
//   [● iPhone 16 · ios-18 ▾] ……………… [Take over | Hand back]  /  [Watching]
// The chip opens the device list (DevicePicker, grouped by platform). The
// device's own controls live on the rail beside the phone
// (MobileControlRail); Stop is in the rail's ⋯. Every control is a gesture
// to the daemon; the bar keeps no device state. A watch-only viewer (remote
// Connect Members, Rosson 2026-10-27) sees the device and "Watching" only.

import { useState } from 'react'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { DevicePicker, type DevicePickerProps } from './DevicePicker'
import { THEME_PILL_RADIUS } from './MobileControlRail'
import type { SimView } from './simApi'

export interface MobileToolbarProps {
  session: SimView | null
  /** The chip's device when nothing is up (the tab's pick). */
  picked: { name: string; os?: string } | null
  /** The device list in the chip's menu. */
  picker: Omit<DevicePickerProps, 'compact'>
  canControl: boolean
  /** The stream is open and drawing (Take over needs it). */
  live: boolean
  /** This viewer holds the device with Take over. */
  tookOver: boolean
  onTakeOver(): void
  onHandBack(): void
}

export function MobileToolbar(props: MobileToolbarProps): React.JSX.Element {
  const { session, canControl, live, tookOver } = props
  const running = session?.state === 'running'
  const changing = session?.state === 'installing' || session?.state === 'booting' || session?.state === 'stopping'
  const up = running || changing
  const shown = up && session?.profile ? { name: session.profile.name, os: session.profile.os } : props.picked
  const [open, setOpen] = useState(false)
  const menu = useAnchoredMenu<HTMLDivElement>({ open, onClose: () => setOpen(false), width: 'min', minWidth: 300, gap: 4 })
  const dot = running
    ? 'bg-[var(--color-status-ok)]'
    : changing
      ? 'animate-pulse bg-[var(--color-status-working)]'
      : 'bg-[var(--color-text-muted)]'
  return (
    <div
      className="flex h-10 flex-shrink-0 items-center gap-1.5 border-b border-[var(--color-border)] px-2 text-xs"
      data-testid="mobile-toolbar"
    >
      <div ref={menu.anchorRef} className="min-w-0">
        <button
          type="button"
          aria-label="Device"
          aria-haspopup="menu"
          aria-expanded={open}
          disabled={!canControl}
          onClick={() => setOpen(!open)}
          title={canControl ? 'Choose a device' : "Your login can't change the device"}
          className="flex h-7 min-w-0 max-w-[18rem] cursor-pointer items-center gap-2 border border-[var(--color-border)] bg-[var(--color-bg)] pl-2.5 pr-2 text-xs text-[var(--color-text-primary)] transition-colors enabled:hover:border-[var(--color-text-muted)] disabled:cursor-default"
          style={{ borderRadius: THEME_PILL_RADIUS }}
          data-mobile-device=""
        >
          <span className={`h-2 w-2 flex-shrink-0 rounded-full ${dot}`} aria-hidden data-mobile-device-state={session?.state ?? 'none'} />
          <span className="truncate" data-mobile-device-label="">
            {shown ? shown.name : 'No devices'}
          </span>
          {shown?.os ? <span className="flex-shrink-0 text-[var(--color-text-muted)]">{shown.os}</span> : null}
          {canControl ? (
            <svg
              className={`h-3 w-3 flex-shrink-0 text-[var(--color-text-muted)] transition-transform ${open ? 'rotate-180' : ''}`}
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
              aria-hidden
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
            </svg>
          ) : null}
        </button>
      </div>
      {open
        ? menu.portal(
            <div
              ref={menu.menuRef}
              style={menu.style}
              className="max-h-[70vh] overflow-y-auto border border-[var(--color-border)] bg-[var(--color-bg)] p-2 shadow-xl"
              role="menu"
              data-mobile-device-menu=""
            >
              <DevicePicker
                {...props.picker}
                compact
                onStart={(platform, id) => {
                  setOpen(false)
                  props.picker.onStart(platform, id)
                }}
                onShowLicence={(platform) => {
                  setOpen(false)
                  props.picker.onShowLicence(platform)
                }}
              />
            </div>,
          )
        : null}
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
