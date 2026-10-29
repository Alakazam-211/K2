// The device list (research-mobile-pane-polish-v1 §4 item 4): every device
// this server can boot, grouped by platform (iPhone, Android), the running
// one first, each with its OS, its state and Start. A lane that isn't ready
// shows what it needs as a card with the one action that resolves it:
//   - a licence → "Show the licence": a boot asks the daemon, which puts
//     the owner's Accept card on this workspace's Thread (beside the
//     device), never "go to Settings";
//   - an owner step → the exact command, with Copy;
//   - a first-use download → said up front (Start does it).
// It shows in the empty state and in the device chip's menu. It reads what
// the daemon already reports (sim/status, sim/devices) and keeps no state.

import { SmartphoneIcon } from '@/components/icons/K2Icons'
import type { SimDeviceProfile, SimLaneNeed, SimPlatform, SimState } from './simApi'

export interface DevicePickerGroup {
  platform: SimPlatform
  devices: SimDeviceProfile[]
  /** A lane for this platform can boot now. */
  ready: boolean
  /** What the platform's lane still needs (licence, owner step, install). */
  needs: SimLaneNeed[]
  /** Still fetching sim/devices for it. */
  loading: boolean
}

export const PLATFORM_LABEL: Record<SimPlatform, string> = { ios: 'iPhone', android: 'Android' }

export interface DevicePickerProps {
  groups: DevicePickerGroup[]
  /** The workspace's device that is up (any state but stopped / failed). */
  up: { id: string; state: SimState } | null
  /** The tab's pick (preselected). */
  selected: string
  /** The device a Start is in flight for. */
  pending: string | null
  canControl: boolean
  busy: boolean
  onStart(platform: SimPlatform, id: string): void
  /** Ask the daemon for the owner's licence card (a boot that needs it). */
  onShowLicence(platform: SimPlatform): void
  /** Copy text (an owner step's command). */
  onCopy(text: string): void
  compact?: boolean
}

function stateLabel(state: SimState): string {
  if (state === 'running') return 'Running'
  if (state === 'stopping') return 'Stopping…'
  if (state === 'installing') return 'Installing tools…'
  return 'Starting…'
}

function formatBytes(n: number): string {
  return n >= 1e9 ? `${(n / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(n / 1e6))} MB`
}

export function DevicePicker(props: DevicePickerProps): React.JSX.Element {
  const { groups, up, selected, pending, canControl, busy } = props
  return (
    <div className="flex w-full flex-col gap-3" data-mobile-picker="">
      {groups.map((g) => {
        const rows = [...g.devices].sort((a, b) => Number(b.id === up?.id) - Number(a.id === up?.id))
        const licence = g.needs.find((n) => n.kind === 'licence')
        const step = g.needs.find((n) => n.kind === 'owner-step')
        const install = g.needs.find((n) => n.kind === 'install')
        return (
          <section key={g.platform} className="flex flex-col gap-1" data-mobile-group={g.platform}>
            <h3 className="px-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">{PLATFORM_LABEL[g.platform]}</h3>
            {licence && licence.kind === 'licence' && !g.ready ? (
              <NeedCard
                title={`Accept the ${licence.title}`}
                body="The server's owner accepts it once. K2 puts the licence on this workspace's Thread to read and accept."
                action={canControl ? { label: 'Show the licence', onClick: () => props.onShowLicence(g.platform) } : null}
                testId="licence"
              />
            ) : null}
            {step && step.kind === 'owner-step' && !g.ready ? (
              <NeedCard
                title={`The owner must ${step.what}`}
                body={<code className="break-all text-[11px] text-[var(--color-text-primary)]">{step.command}</code>}
                action={{ label: 'Copy', onClick: () => props.onCopy(step.command) }}
                testId="owner-step"
              />
            ) : null}
            {install && install.kind === 'install' && g.ready ? (
              <div className="px-1 text-[11px] text-[var(--color-text-muted)]" data-mobile-need="install">
                The first start downloads {install.what}
                {install.bytes ? ` (about ${formatBytes(install.bytes)})` : ''}.
              </div>
            ) : null}
            {g.loading && rows.length === 0 ? (
              <div className="px-1 text-xs text-[var(--color-text-muted)]">Loading devices…</div>
            ) : null}
            {!g.loading && rows.length === 0 ? (
              <div className="px-1 text-xs text-[var(--color-text-muted)]">No devices</div>
            ) : null}
            {rows.map((d) => {
              const isUp = up?.id === d.id
              const starting = pending === d.id
              const sub = [d.os, isUp && up ? stateLabel(up.state) : starting ? 'Starting…' : 'Stopped'].join(' · ')
              return (
                <div
                  key={d.id}
                  className={`flex items-center gap-2.5 border px-2 ${props.compact ? 'py-1.5' : 'py-2'} ${
                    selected === d.id ? 'border-[var(--color-accent)]/60 bg-[var(--color-accent)]/5' : 'border-[var(--color-border)]'
                  }`}
                  style={{ borderRadius: 'var(--radius-field, 0px)' }}
                  data-mobile-device-row={d.id}
                >
                  <span
                    className={`flex h-7 w-7 flex-shrink-0 items-center justify-center bg-[var(--color-bg-elevated)] ${
                      isUp ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-secondary)]'
                    }`}
                    style={{ borderRadius: 'var(--radius-field, 0px)' }}
                    aria-hidden
                  >
                    <SmartphoneIcon size={14} />
                  </span>
                  <span className="flex min-w-0 flex-1 flex-col">
                    <span className="flex items-center gap-1.5 truncate text-xs text-[var(--color-text-primary)]">
                      <span className="truncate" data-mobile-device-name="">{d.name}</span>
                      {d.default ? (
                        <span className="flex-shrink-0 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">Default</span>
                      ) : null}
                    </span>
                    <span className="truncate text-[11px] text-[var(--color-text-muted)]">{sub}</span>
                  </span>
                  {isUp ? (
                    <span className="flex flex-shrink-0 items-center gap-1.5 text-[11px] text-[var(--color-text-secondary)]">
                      <span className={`h-1.5 w-1.5 rounded-full ${up?.state === 'running' ? 'bg-[var(--color-status-ok)]' : 'animate-pulse bg-[var(--color-status-working)]'}`} aria-hidden />
                      {up?.state === 'running' ? 'On' : '…'}
                    </span>
                  ) : canControl ? (
                    <button
                      type="button"
                      className="flex h-6 flex-shrink-0 cursor-pointer items-center gap-1.5 border border-[var(--color-border)] px-2.5 text-xs text-[var(--color-text-primary)] enabled:hover:border-[var(--color-accent)] disabled:cursor-default disabled:opacity-45"
                      style={{ borderRadius: 'var(--radius-field, 0px)' }}
                      disabled={!g.ready || busy || pending !== null}
                      title={g.ready ? `Start ${d.name}` : 'This platform needs setting up first'}
                      onClick={() => props.onStart(g.platform, d.id)}
                      data-mobile-start={d.id}
                    >
                      {starting ? (
                        <span className="h-3 w-3 animate-spin rounded-full border-2 border-[var(--color-accent)] border-t-transparent" aria-hidden />
                      ) : null}
                      Start
                    </button>
                  ) : null}
                </div>
              )
            })}
          </section>
        )
      })}
    </div>
  )
}

function NeedCard({
  title,
  body,
  action,
  testId,
}: {
  title: string
  body: React.ReactNode
  action: { label: string; onClick(): void } | null
  testId: string
}): React.JSX.Element {
  return (
    <div
      className="flex flex-col gap-1.5 border border-[var(--color-status-warn)] bg-[var(--color-status-warn-soft)] px-3 py-2 text-xs"
      style={{ borderRadius: 'var(--radius-field, 0px)' }}
      data-mobile-need={testId}
    >
      <div className="text-[var(--color-text-primary)]">{title}</div>
      <div className="text-[11px] text-[var(--color-text-secondary)]">{body}</div>
      {action ? (
        <button
          type="button"
          className="h-6 cursor-pointer self-start border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 text-xs text-[var(--color-text-primary)] hover:border-[var(--color-accent)]"
          style={{ borderRadius: 'var(--radius-field, 0px)' }}
          onClick={action.onClick}
          data-mobile-need-action={testId}
        >
          {action.label}
        </button>
      ) : null}
    </div>
  )
}
