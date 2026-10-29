// The control rail beside the device (research-mobile-pane-polish-v1 §4
// item 2): a vertical pill to the right of the phone, top to bottom:
//   Home · Back + Recents (Android) │ Rotate · Light/Dark · Screenshot │ ⋯
// ⋯ holds the rare and destructive ones: Open URL…, Frame on/off, the
// frame rate (remote), About this device (with the isolation note), and
// Stop the device in red.
//
// Every control is a gesture the daemon already takes (stream::ViewerMsg:
// button, rotate, appearance, open-url; sim/stop, sim/screenshot). The rail
// keeps no device state. Its corners follow the theme: square on K2's
// square theme, a pill where the theme rounds (`--radius-field`).

import { useState } from 'react'
import {
  ArrowLeftIcon,
  CameraIcon,
  HomeIcon,
  K2_ICON_PX,
  MoonIcon,
  MoreIcon,
  RecentsIcon,
  RotateIcon,
  SunIcon,
} from '@/components/icons/K2Icons'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import type { SimLaneId } from './simApi'
import { REMOTE_FPS_CHOICES, type SimAppearance } from './sim-stream'

/** The rail's width; the phone fits in what is left beside it. */
export const RAIL_INSET_PX = 56

/** 0 on a square theme, a pill where the theme rounds its fields. */
export const THEME_PILL_RADIUS = 'calc(var(--radius-field, 0px) * 100)'

const WAIT_SCREEN = "Waits for the device's screen"

export interface MobileControlRailProps {
  lane: SimLaneId | null
  /** The stream is open and drawing (the device's buttons need it). */
  live: boolean
  /** A stop / screenshot request is in flight. */
  busy: boolean
  /** What this tab last set (the daemon doesn't report it). */
  appearance: SimAppearance
  screenOnly: boolean
  /** Remote viewers pick their frame rate; null = local. */
  fps: number | null
  deviceName: string
  deviceOs?: string
  isolation: string | null
  onHome(): void
  onBack(): void
  onRecents(): void
  onRotate(): void
  onAppearance(next: SimAppearance): void
  onScreenshot(): void
  onStop(): void
  onOpenUrl(url: string): void
  onScreenOnly(on: boolean): void
  onFps(fps: number): void
  /** S5: the Select toggle. */
  extra?: React.ReactNode
}

export function RailButton({
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
      className={`flex h-7 w-7 flex-shrink-0 items-center justify-center transition-colors duration-150 cursor-pointer disabled:cursor-default disabled:opacity-35 ${
        pressed
          ? 'bg-[var(--color-accent)]/15 text-[var(--color-accent)]'
          : 'text-[var(--color-text-secondary)] enabled:hover:bg-[var(--color-bg-elevated)] enabled:hover:text-[var(--color-text-primary)]'
      }`}
      style={{ borderRadius: THEME_PILL_RADIUS }}
    >
      {children}
    </button>
  )
}

const RailDivider = (): React.JSX.Element => <span className="my-1 h-px w-4 flex-shrink-0 bg-[var(--color-border)]" aria-hidden />

export function MobileControlRail(props: MobileControlRailProps): React.JSX.Element {
  const { lane, live, busy, appearance } = props
  const [open, setOpen] = useState(false)
  const [urlOpen, setUrlOpen] = useState(false)
  const [url, setUrl] = useState('')
  const close = (): void => {
    setOpen(false)
    setUrlOpen(false)
  }
  const menu = useAnchoredMenu<HTMLDivElement>({ open, onClose: close, width: 'min', minWidth: 232, align: 'end', gap: 6 })
  const dark = appearance === 'dark'
  const android = lane === 'android'
  const sendUrl = (): void => {
    const u = url.trim()
    if (!u) return
    props.onOpenUrl(u)
    setUrl('')
    close()
  }
  const item =
    'flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs cursor-pointer text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)]'
  return (
    <div
      className="flex flex-col items-center gap-0.5 border border-[var(--color-border)] bg-[var(--color-bg)]/85 px-1 py-1.5 shadow-md backdrop-blur"
      style={{ borderRadius: THEME_PILL_RADIUS }}
      role="toolbar"
      aria-orientation="vertical"
      aria-label="Device controls"
      data-mobile-rail=""
    >
      <RailButton label="Home" title={live ? 'Home (⌘⇧H on the device)' : WAIT_SCREEN} onClick={props.onHome} disabled={!live} testId="home">
        <HomeIcon size={K2_ICON_PX} />
      </RailButton>
      {android ? (
        <>
          <RailButton label="Back" title={live ? 'Back' : WAIT_SCREEN} onClick={props.onBack} disabled={!live} testId="back">
            <ArrowLeftIcon size={K2_ICON_PX} />
          </RailButton>
          <RailButton label="Recents" title={live ? 'Recent apps' : WAIT_SCREEN} onClick={props.onRecents} disabled={!live} testId="recents">
            <RecentsIcon size={K2_ICON_PX} />
          </RailButton>
        </>
      ) : null}
      <RailDivider />
      <RailButton label="Rotate" title={live ? 'Rotate' : WAIT_SCREEN} onClick={props.onRotate} disabled={!live} testId="rotate">
        <RotateIcon size={K2_ICON_PX} />
      </RailButton>
      <RailButton
        label="Dark mode"
        title={live ? (dark ? 'Switch the device to light mode' : 'Switch the device to dark mode') : WAIT_SCREEN}
        onClick={() => props.onAppearance(dark ? 'light' : 'dark')}
        disabled={!live}
        pressed={dark}
        testId="appearance"
      >
        {dark ? <MoonIcon size={K2_ICON_PX} /> : <SunIcon size={K2_ICON_PX} />}
      </RailButton>
      <RailButton
        label="Screenshot"
        title={busy ? 'Busy…' : 'Save a screenshot to .k2/sim/shots/'}
        onClick={props.onScreenshot}
        disabled={busy}
        testId="screenshot"
      >
        <CameraIcon size={K2_ICON_PX} />
      </RailButton>
      {props.extra}
      <RailDivider />
      <div ref={menu.anchorRef}>
        <RailButton label="More" title="More device actions" onClick={() => (open ? close() : setOpen(true))} pressed={open} testId="more">
          <MoreIcon size={K2_ICON_PX} />
        </RailButton>
      </div>
      {open
        ? menu.portal(
            <div
              ref={menu.menuRef}
              style={menu.style}
              className="border border-[var(--color-border)] bg-[var(--color-bg)] py-1 shadow-xl"
              role="menu"
              data-mobile-more=""
            >
              {urlOpen ? (
                <div className="flex items-center gap-1 px-2 py-1">
                  <input
                    autoFocus
                    type="url"
                    value={url}
                    placeholder="https:// or myapp://"
                    onChange={(e) => setUrl(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault()
                        sendUrl()
                      }
                    }}
                    className="h-6 min-w-0 flex-1 border border-[var(--color-border)] bg-[var(--color-bg)] px-1.5 text-xs text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)]"
                    aria-label="URL to open on the device"
                    data-mobile-open-url=""
                  />
                  <button
                    type="button"
                    className="h-6 cursor-pointer border border-[var(--color-border)] px-2 text-xs text-[var(--color-text-primary)] hover:border-[var(--color-accent)] disabled:cursor-default disabled:opacity-50"
                    disabled={!url.trim() || !live}
                    onClick={sendUrl}
                    data-mobile-open-url-go=""
                  >
                    Open
                  </button>
                </div>
              ) : (
                <button type="button" role="menuitem" className={item} disabled={!live} onClick={() => setUrlOpen(true)} data-mobile-menu="open-url">
                  Open URL…
                </button>
              )}
              <button
                type="button"
                role="menuitemcheckbox"
                aria-checked={!props.screenOnly}
                className={item}
                onClick={() => {
                  props.onScreenOnly(!props.screenOnly)
                  close()
                }}
                data-mobile-menu="frame"
              >
                <span className="flex-1">Show the phone frame</span>
                {!props.screenOnly ? <Check /> : null}
              </button>
              {props.fps !== null ? (
                <div className="border-t border-[var(--color-border)] pt-1 mt-1" data-mobile-fps="">
                  <div className="px-3 pb-0.5 pt-1 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">Frame rate over Connect</div>
                  {REMOTE_FPS_CHOICES.map((n) => (
                    <button
                      key={n}
                      type="button"
                      role="menuitemradio"
                      aria-checked={props.fps === n}
                      className={`${item} ${props.fps === n ? 'text-[var(--color-accent)]' : ''}`}
                      onClick={() => {
                        props.onFps(n)
                        close()
                      }}
                      data-mobile-fps-choice={n}
                    >
                      <span className="flex-1">{n} fps</span>
                      {props.fps === n ? <Check /> : null}
                    </button>
                  ))}
                </div>
              ) : null}
              <div className="mt-1 border-t border-[var(--color-border)] px-3 py-2 text-[11px] leading-snug text-[var(--color-text-muted)]" data-mobile-about="">
                <div className="text-xs text-[var(--color-text-primary)]">{props.deviceName}</div>
                {props.deviceOs ? <div>{props.deviceOs}</div> : null}
                {props.isolation ? <div className="mt-1">{props.isolation}</div> : null}
              </div>
              <div className="border-t border-[var(--color-border)] pt-1">
                <button
                  type="button"
                  role="menuitem"
                  className="flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-left text-xs text-[var(--color-danger)] hover:bg-[var(--color-danger)]/10 disabled:cursor-default disabled:opacity-50"
                  disabled={busy}
                  onClick={() => {
                    props.onStop()
                    close()
                  }}
                  data-mobile-tool="stop"
                >
                  Stop the device
                </button>
              </div>
            </div>,
          )
        : null}
    </div>
  )
}

function Check(): React.JSX.Element {
  return (
    <svg className="h-3 w-3 flex-shrink-0 text-[var(--color-accent)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5} aria-hidden>
      <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
    </svg>
  )
}
