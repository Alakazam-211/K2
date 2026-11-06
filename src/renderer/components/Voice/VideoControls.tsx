// Video in voice rooms: the call panel's video controls. Camera on/off, share
// screen (with its picker), the camera picker beside Microphone and Speakers,
// and the line that says why video was refused.
//
// Everything here takes the controller as a prop, so the dev video demo can run
// it over the fake engine.

import { useEffect } from 'react'
import { useStore } from 'zustand'
import { SettingDropdown } from '@/components/Settings/controls/SettingControls'
import type { VideoController } from '@/stores/video-core'

const SMALL_BTN =
  'border border-[var(--color-border)] px-2 py-0.5 text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] disabled:opacity-50'

/** Camera and Share screen, beside Mute and Deafen. */
export function VideoCallButtons({ ctl, disabled = false }: { ctl: VideoController; disabled?: boolean }): React.JSX.Element | null {
  const availability = useStore(ctl.store, (s) => s.availability)
  const caps = useStore(ctl.store, (s) => s.caps)
  const local = useStore(ctl.store, (s) => s.local)
  const pending = useStore(ctl.store, (s) => s.pending)
  if (availability !== 'ready' || !caps) return null
  const cameraOn = local.camera !== 'off'
  const screenOn = local.screen !== 'off'
  const share = (): void => {
    if (screenOn) {
      void ctl.setScreen(false)
      return
    }
    // A listed source needs our picker; otherwise the system shows its own.
    if (caps.screen.list && !caps.screen.systemPicker) void ctl.loadScreenSources()
    else void ctl.setScreen(true)
  }
  return (
    <>
      {caps.camera ? (
        <button
          type="button"
          data-testid="video-camera"
          data-on={cameraOn ? 'true' : 'false'}
          disabled={disabled || pending.camera || local.camera === 'asking'}
          className={SMALL_BTN}
          onClick={() => void ctl.setCamera(!cameraOn)}
        >
          {cameraOn ? 'Camera off' : 'Camera on'}
        </button>
      ) : null}
      {caps.screen.supported ? (
        <button
          type="button"
          data-testid="video-screen"
          data-on={screenOn ? 'true' : 'false'}
          disabled={disabled || pending.screen || local.screen === 'asking'}
          className={SMALL_BTN}
          onClick={share}
        >
          {screenOn ? 'Stop sharing' : 'Share screen'}
        </button>
      ) : null}
    </>
  )
}

/** What a share can show: screens first, then windows (where the OS lists them). */
export function ScreenPicker({ ctl }: { ctl: VideoController }): React.JSX.Element | null {
  const sources = useStore(ctl.store, (s) => s.screenSources)
  if (sources === null) return null
  return (
    <div data-testid="video-screen-picker" className="mt-2 flex items-center justify-between gap-2 text-[11px] text-[var(--color-text-secondary)]">
      {sources.length === 0 ? (
        <span>Nothing to share was found.</span>
      ) : (
        <>
          Share
          <SettingDropdown
            compact
            className="min-w-0 max-w-[170px]"
            ariaLabel="Screen or window to share"
            triggerData={{ 'data-testid': 'video-screen-source' }}
            placeholder="Pick a screen or window"
            value=""
            options={sources.map((s) => ({
              value: s.id,
              label: s.app && s.kind === 'window' ? `${s.title} · ${s.app}` : s.title,
              group: s.kind === 'window' ? 'Windows' : 'Screens',
              leading: s.thumbnail ? <img src={s.thumbnail} alt="" className="h-5 w-8 object-cover" /> : undefined,
            }))}
            onChange={(id) => void ctl.setScreen(true, id)}
          />
        </>
      )}
      <button type="button" data-testid="video-screen-cancel" className={SMALL_BTN} onClick={() => ctl.closeScreenPicker()}>
        Cancel
      </button>
    </div>
  )
}

/** The Camera row, under Microphone and Speakers. */
export function CameraPicker({ ctl }: { ctl: VideoController }): React.JSX.Element | null {
  const availability = useStore(ctl.store, (s) => s.availability)
  const devices = useStore(ctl.store, (s) => s.devices)
  const caps = useStore(ctl.store, (s) => s.caps)
  const cameraId = useStore(ctl.store, (s) => s.cameraId)
  useEffect(() => {
    if (availability !== 'ready') return
    void ctl.loadDevices().catch((err: unknown) => console.warn('[video] reading cameras failed:', err))
  }, [ctl, availability])
  if (availability !== 'ready' || !caps?.camera || !devices || devices.cameras.length === 0) return null
  return (
    <div className="flex items-center justify-between gap-2">
      Camera
      <SettingDropdown
        compact
        className="min-w-0 max-w-[170px]"
        ariaLabel="Camera"
        triggerData={{ 'data-testid': 'video-camera-input' }}
        value={cameraId ?? ''}
        options={[{ value: '', label: 'System default' }, ...devices.cameras.map((c) => ({ value: c.id, label: c.label }))]}
        onChange={(v) => void ctl.setCameraDevice(v || null)}
      />
    </div>
  )
}

/** Why video didn't start, and the way to the OS's privacy pane when that's why. */
export function VideoNoticeLine({ ctl }: { ctl: VideoController }): React.JSX.Element | null {
  const notice = useStore(ctl.store, (s) => s.notice)
  const devices = useStore(ctl.store, (s) => s.devices)
  const availability = useStore(ctl.store, (s) => s.availability)
  const cameraBlocked = devices?.permission.camera === 'denied'
  if (availability === 'unavailable') {
    return (
      <p data-testid="video-unavailable" className="mt-1 text-[10px] text-[var(--color-text-muted)]">
        Video isn't in this version of K2 yet.
      </p>
    )
  }
  const text = notice ?? (cameraBlocked ? { line: "K2 isn't allowed to use your camera.", kind: 'error' as const, settings: 'camera' as const } : null)
  if (!text) return null
  return (
    <p
      data-testid="video-notice"
      data-kind={text.kind}
      className={`mt-1 text-[11px] ${text.kind === 'error' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-text-secondary)]'}`}
    >
      {text.line}
      {text.settings ? (
        <>
          {' '}
          <button
            type="button"
            data-testid="video-privacy-settings"
            className="underline hover:text-[var(--color-text-primary)]"
            onClick={() => void ctl.openPrivacySettings(text.settings as 'camera' | 'screen')}
          >
            Open privacy settings
          </button>
        </>
      ) : null}
    </p>
  )
}
