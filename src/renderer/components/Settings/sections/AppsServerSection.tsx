// Settings → Apps → This server (0.46.3, "Apps on for real"; Rosson
// 2026-10-20). Set up Apps on the window's server from the desktop app, with
// no terminal on the box: the owner logs in over K2 Connect (or is local)
// and everything here goes to that server (`primaryScope()`).
//
//   A. The Apps (beta) switch: the `apps` setting, owner-only (the daemon
//      403s anyone else; the switch is shown disabled with the reason).
//   B. The box doctor (GET /cli/app/box): each check ok / warn / fail with
//      its fix.
//   C. Prepare this box: a dry run first, every change listed, then an
//      explicit Apply (with K2's Podman when the dry run says it's needed).
//   D. Install App engine (the guarded engine, through the root helper).
//   E. Undo box setup (owner): a dry run lists what it undoes and keeps,
//      then an explicit second click undoes it; refused while Apps run.
//      Automatic prep pauses until Retry (a stopped automatic prep shows
//      why and the same Retry).
//
// Which agents may manage Apps stays on the Agent tab (a separate slice
// reshapes those switches); this panel is independent of them.
//
// Thin client, daemon-first: the daemon owns the switch, the doctor and what
// prepare does; this panel reads that truth and sends gestures. No polling:
// it re-reads on open, on a server switch, after each gesture and when the
// window regains focus. No fetchProjects().

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Callout, Toggle } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import { useSettingsStore } from '@/stores/settings'
import { useConnectHostStore } from '@/stores/connect-host'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { SettingsGroup } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import {
  changeLines,
  checkLabel,
  engineNeeded,
  fetchAppBox,
  installEngine,
  isAppsOff,
  needsPodmanInstall,
  prepareApply,
  prepareDryRun,
  prepareNeeded,
  retryAutoPrepare,
  setAutoPrepare,
  unprepApply,
  unprepDryRun,
  type BoxCheck,
  type BoxDoctor,
  type PrepareDryRun,
  type UnprepPlan,
} from './apps-server-api'

export const APPS_SERVER_MANIFEST: SettingEntry[] = [
  {
    id: 'skin-access.server',
    section: 'skin-access',
    label: 'Apps (beta) on this server',
    description: 'Turn Apps on or off, check the box, prepare it and install the App engine',
    keywords: ['apps', 'app', 'beta', 'this server', 'doctor', 'prepare', 'box', 'podman', 'engine', 'k2 app doctor', 'setup'],
    group: 'This server',
  },
]

const muted = 'text-[var(--color-text-muted)]'
const secondary = 'text-[var(--color-text-secondary)]'

type BoxState = { kind: 'loading' } | { kind: 'off' } | { kind: 'ready'; doctor: BoxDoctor } | { kind: 'error'; text: string }

type Notice = { tone: 'ok' | 'error' | 'info' | 'warn'; text: string }

const STATUS_STYLE: Record<string, string> = {
  ok: 'text-[var(--color-status-ok-soft)]',
  warn: 'text-[var(--color-status-warn-amber-soft)]',
  fail: 'text-[var(--color-status-error-soft)]',
  unknown: 'text-[var(--color-text-muted)]',
}

export function AppsServerSection(): React.JSX.Element {
  const apps = useSettingsStore((s) => s.apps)
  const setApps = useSettingsStore((s) => s.setApps)
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const [role, setRole] = useState<string | null>(null)
  const [box, setBox] = useState<BoxState>({ kind: 'loading' })
  const [switchBusy, setSwitchBusy] = useState(false)
  const [notice, setNotice] = useState<Notice | null>(null)
  const [dry, setDry] = useState<PrepareDryRun | null>(null)
  const [unprep, setUnprep] = useState<UnprepPlan | null>(null)
  const [busy, setBusy] = useState<
    null | 'dry' | 'apply' | 'engine' | 'auto' | 'retry' | 'unprep-dry' | 'unprep'
  >(null)
  const mounted = useRef(true)

  const isOwner = role === 'owner'

  const load = useCallback(async (): Promise<void> => {
    try {
      const d = await fetchAppBox()
      if (mounted.current) setBox({ kind: 'ready', doctor: d })
    } catch (e) {
      if (!mounted.current) return
      setBox(isAppsOff(e) ? { kind: 'off' } : { kind: 'error', text: daemonErrorText(e) })
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    setBox({ kind: 'loading' })
    setDry(null)
    setUnprep(null)
    setNotice(null)
    void load()
    void fetchViewerRole().then((r) => {
      if (mounted.current) setRole(r)
    })
    const onFocus = (): void => void load()
    window.addEventListener('focus', onFocus)
    return () => {
      mounted.current = false
      window.removeEventListener('focus', onFocus)
    }
  }, [load, activeHost])

  const flipApps = async (next: boolean): Promise<void> => {
    setSwitchBusy(true)
    setNotice(null)
    try {
      await setApps(next)
      setDry(null)
      await load()
    } catch (e) {
      setNotice({ tone: 'error', text: daemonErrorText(e) })
    } finally {
      setSwitchBusy(false)
    }
  }

  const runDry = async (): Promise<void> => {
    setBusy('dry')
    setNotice(null)
    try {
      setDry(await prepareDryRun())
    } catch (e) {
      setNotice({ tone: 'error', text: daemonErrorText(e) })
    } finally {
      setBusy(null)
    }
  }

  const apply = async (): Promise<void> => {
    if (!dry) return
    setBusy('apply')
    setNotice(null)
    try {
      const r = await prepareApply(needsPodmanInstall(dry))
      setDry(null)
      if (r.box) setBox({ kind: 'ready', doctor: r.box })
      setNotice({ tone: 'ok', text: 'The box is prepared.' })
      await load()
    } catch (e) {
      setNotice({
        tone: 'error',
        text: `${daemonErrorText(e)} (the checks below show where the box is now)`,
      })
      await load()
    } finally {
      setBusy(null)
    }
  }

  const flipAuto = async (next: boolean): Promise<void> => {
    setBusy('auto')
    setNotice(null)
    try {
      await setAutoPrepare(next)
      await load()
    } catch (e) {
      setNotice({ tone: 'error', text: daemonErrorText(e) })
    } finally {
      setBusy(null)
    }
  }

  const retryAuto = async (): Promise<void> => {
    setBusy('retry')
    setNotice(null)
    try {
      await retryAutoPrepare()
      setNotice({ tone: 'info', text: 'Automatic prep will run again now.' })
      await load()
    } catch (e) {
      setNotice({ tone: 'error', text: daemonErrorText(e) })
    } finally {
      setBusy(null)
    }
  }

  const unprepPlan = async (): Promise<void> => {
    setBusy('unprep-dry')
    setNotice(null)
    try {
      setUnprep(await unprepDryRun())
    } catch (e) {
      setNotice({ tone: 'error', text: daemonErrorText(e) })
    } finally {
      setBusy(null)
    }
  }

  const unprepConfirm = async (): Promise<void> => {
    if (!unprep) return
    setBusy('unprep')
    setNotice(null)
    try {
      const r = await unprepApply()
      setUnprep(null)
      setNotice({
        tone: 'ok',
        text: `The box setup is undone.${r.note ? ` ${r.note}` : ''}`,
      })
      await load()
    } catch (e) {
      setNotice({ tone: 'error', text: `${daemonErrorText(e)} (the checks below show where the box is now)` })
      await load()
    } finally {
      setBusy(null)
    }
  }

  const engine = async (): Promise<void> => {
    setBusy('engine')
    setNotice(null)
    try {
      const r = await installEngine()
      setNotice({ tone: 'ok', text: `The App engine ${r.version ?? ''} is installed and answering.`.replace('  ', ' ') })
      await load()
    } catch (e) {
      setNotice({ tone: 'error', text: daemonErrorText(e) })
      await load()
    } finally {
      setBusy(null)
    }
  }

  const doctor = box.kind === 'ready' ? box.doctor : null
  const appsOn = box.kind === 'off' ? false : doctor ? doctor.apps : apps

  return (
    <div className="max-w-2xl space-y-6" data-testid="apps-server" data-settings-id="skin-access.server">
      <div>
        <h2 className="text-sm font-medium text-[var(--color-text-primary)]">This server</h2>
        <p className={`text-[11px] ${muted} mt-1 leading-relaxed`}>
          Apps your agents build run on this server, each in its own wall. Set the server up here: no terminal on the
          box needed.
        </p>
      </div>

      <SettingsGroup title="Apps (beta)">
        <div className="flex items-start gap-3 py-2">
          <Toggle
            checked={appsOn}
            onChange={(v) => void flipApps(v)}
            disabled={!isOwner || switchBusy}
            aria-label="Apps (beta)"
          />
          <div className="flex-1 min-w-0">
            <div className="text-xs text-[var(--color-text-primary)]">Apps (beta)</div>
            <p className={`text-[11px] ${muted} mt-0.5`}>
              On: agents you allow can build, install and deploy Apps here. Apps run walled, in a guarded container. Off:
              every App route is closed.
            </p>
            {!isOwner && role !== null ? (
              <p className={`text-[11px] ${secondary} mt-1`} data-testid="apps-switch-owner-only">
                Only the server&apos;s owner can turn Apps on or off.
              </p>
            ) : null}
          </div>
        </div>
      </SettingsGroup>

      {notice ? (
        <Callout tone={notice.tone} data-testid="apps-server-notice">
          {notice.text}
        </Callout>
      ) : null}

      {box.kind === 'loading' ? <p className={`text-[11px] ${muted}`}>Checking the box…</p> : null}
      {box.kind === 'error' ? <Callout tone="error">{box.text}</Callout> : null}
      {box.kind === 'off' ? (
        <p className={`text-[11px] ${muted}`} data-testid="apps-off">
          Apps are off on this server. Turn them on to check and prepare the box.
        </p>
      ) : null}

      {doctor ? (
        <>
          <SettingsGroup title="Box checks">
            <div className="py-2 space-y-2" data-testid="apps-doctor">
              <p className="text-[11px]">
                <span className={doctor.ready ? STATUS_STYLE.ok : STATUS_STYLE.warn}>
                  {doctor.ready ? 'Ready for walled Apps' : 'Not ready yet'}
                </span>
                {doctor.runtime ? <span className={muted}> · runtime {doctor.runtime}</span> : null}
              </p>
              {!doctor.linux ? (
                <p className={`text-[11px] ${muted}`}>
                  Apps run walled on Linux servers. This machine can build and check Apps, not run them.
                </p>
              ) : null}
              <ul className="space-y-1.5">
                {doctor.checks.map((c) => (
                  <CheckRow key={c.id} check={c} />
                ))}
              </ul>
              <button
                type="button"
                className={`text-[11px] ${secondary} underline no-drag cursor-pointer`}
                onClick={() => void load()}
              >
                Check again
              </button>
            </div>
          </SettingsGroup>

          {doctor.linux ? (
            <SettingsGroup title="Set up the box">
              <div className="py-2 space-y-3" data-testid="apps-setup">
                {!isOwner && role !== null ? (
                  <p className={`text-[11px] ${secondary}`}>Only the server&apos;s owner can prepare the box or install the engine.</p>
                ) : null}
                {doctor.autoPrepare ? (
                  <div className="flex items-start gap-3" data-testid="apps-auto-prepare">
                    <Toggle
                      checked={doctor.autoPrepare.enabled}
                      onChange={(v) => void flipAuto(v)}
                      disabled={!isOwner || busy !== null}
                      aria-label="Prepare this box automatically"
                    />
                    <div className="flex-1 min-w-0">
                      <div className="text-xs text-[var(--color-text-primary)]">Prepare this box automatically</div>
                      <p className={`text-[11px] ${muted} mt-0.5`}>
                        After each K2 update, K2 sets the box up for walled Apps by itself through its root helper (the
                        steps below, plus K2&apos;s Podman where needed and the App engine), so agents you allow can
                        deploy with nobody running sudo. It stops on a box whose sub-UIDs already overlap (rootless
                        Docker) and tells you what it changed.
                      </p>
                      {doctor.autoPrepare.state ? (
                        <p className={`text-[11px] ${secondary} mt-1`} data-testid="apps-auto-prepare-last">
                          Last run: {doctor.autoPrepare.state}
                          {doctor.autoPrepare.detail ? ` — ${doctor.autoPrepare.detail}` : ''}
                        </p>
                      ) : null}
                      {doctor.autoPrepare.undoneAt !== null || doctor.autoPrepare.blocked ? (
                        <div className="mt-1 space-y-1" data-testid="apps-auto-prepare-stopped">
                          <p className={`text-[11px] ${STATUS_STYLE.warn}`}>
                            {doctor.autoPrepare.undoneAt !== null
                              ? 'Paused because you undid the box setup.'
                              : `Stopped: ${doctor.autoPrepare.detail ?? doctor.autoPrepare.code ?? 'this box can’t be prepared'}.`}{' '}
                            K2 won&apos;t try again until you press Retry or K2 updates.
                          </p>
                          <Button
                            onClick={() => void retryAuto()}
                            disabled={!isOwner || busy !== null}
                            data-testid="apps-auto-prepare-retry"
                          >
                            {busy === 'retry' ? 'Retrying…' : 'Retry'}
                          </Button>
                        </div>
                      ) : null}
                    </div>
                  </div>
                ) : null}
                <div className="space-y-2">
                  <div className="text-xs text-[var(--color-text-primary)]">Prepare this box</div>
                  <p className={`text-[11px] ${muted}`}>
                    Sets up the k2app user, its limits, the firewall table and the container runtime through K2&apos;s root
                    helper. See every change first; nothing changes until you apply.
                    {prepareNeeded(doctor) ? '' : ' The checks say the box is prepared; running it again is safe.'}
                  </p>
                  {dry ? (
                    <div className="space-y-2" data-testid="apps-dry-run">
                      <div className={`text-[11px] ${secondary}`}>Apply will make these changes, in order:</div>
                      <ol className="list-decimal ml-5 space-y-0.5">
                        {changeLines(dry).map((l, i) => (
                          <li key={i} className="text-[11px] font-mono text-[var(--color-text-primary)] break-all">
                            {l}
                          </li>
                        ))}
                      </ol>
                      {needsPodmanInstall(dry) && dry.podman.bundle ? (
                        <p className={`text-[11px] ${muted} break-all`}>
                          K2&apos;s Podman {dry.podman.bundle.version}, sha256 {dry.podman.bundle.sha256}, from{' '}
                          {dry.podman.bundle.url}
                        </p>
                      ) : null}
                      <div className="flex gap-2">
                        <Button
                          variant="accent"
                          onClick={() => void apply()}
                          disabled={!isOwner || busy !== null}
                          data-testid="apps-prepare-apply"
                        >
                          {busy === 'apply' ? 'Applying… (this can take a few minutes)' : needsPodmanInstall(dry) ? 'Apply (installs Podman)' : 'Apply'}
                        </Button>
                        <Button variant="ghost" onClick={() => setDry(null)} disabled={busy !== null}>
                          Cancel
                        </Button>
                      </div>
                    </div>
                  ) : (
                    <Button
                      onClick={() => void runDry()}
                      disabled={busy !== null}
                      data-testid="apps-prepare-dry-run"
                    >
                      {busy === 'dry' ? 'Checking…' : 'Prepare this box…'}
                    </Button>
                  )}
                </div>
                <div className="space-y-2 pt-2 border-t border-[var(--color-border)]">
                  <div className="text-xs text-[var(--color-text-primary)]">Install App engine</div>
                  <p className={`text-[11px] ${muted}`}>
                    The guarded engine runs each App in its own rootless container. It is signed with K2&apos;s release key
                    and matches this server&apos;s K2 version.
                    {engineNeeded(doctor) ? '' : ' It is installed and answering.'}
                  </p>
                  <Button
                    onClick={() => void engine()}
                    disabled={!isOwner || busy !== null}
                    data-testid="apps-engine-install"
                  >
                    {busy === 'engine' ? 'Installing…' : engineNeeded(doctor) ? 'Install App engine' : 'Reinstall App engine'}
                  </Button>
                </div>
                <div className="space-y-2 pt-2 border-t border-[var(--color-border)]" data-testid="apps-unprep">
                  <div className="text-xs text-[var(--color-text-primary)]">Undo box setup</div>
                  <p className={`text-[11px] ${muted}`}>
                    Removes what box prep set up (the firewall table, AppArmor profile, limits, linger and the engine
                    unit). The k2app user, its data and the distro packages stay. K2&apos;s Podman stays too (k2 app box
                    podman uninstall removes it). Automatic prep pauses until you press Retry. Refused while any App runs.
                  </p>
                  {unprep ? (
                    <div className="space-y-2" data-testid="apps-unprep-plan">
                      <div className={`text-[11px] ${secondary}`}>Undo will remove:</div>
                      <ul className="list-disc ml-5 space-y-0.5">
                        {unprep.undoes.map((l, i) => (
                          <li key={i} className="text-[11px] font-mono text-[var(--color-text-primary)] break-all">
                            {l}
                          </li>
                        ))}
                      </ul>
                      {unprep.kept.length ? (
                        <p className={`text-[11px] ${muted}`}>Kept: {unprep.kept.join('; ')}</p>
                      ) : null}
                      {unprep.runningApps.length ? (
                        <p className={`text-[11px] ${STATUS_STYLE.warn}`} data-testid="apps-unprep-running">
                          Stop these Apps first: {unprep.runningApps.join(', ')}
                        </p>
                      ) : null}
                      <div className="flex gap-2">
                        <Button
                          variant="danger"
                          onClick={() => void unprepConfirm()}
                          disabled={!isOwner || busy !== null || unprep.runningApps.length > 0}
                          data-testid="apps-unprep-confirm"
                        >
                          {busy === 'unprep' ? 'Undoing…' : 'Undo box setup'}
                        </Button>
                        <Button variant="ghost" onClick={() => setUnprep(null)} disabled={busy !== null}>
                          Cancel
                        </Button>
                      </div>
                    </div>
                  ) : (
                    <Button
                      onClick={() => void unprepPlan()}
                      disabled={!isOwner || busy !== null}
                      data-testid="apps-unprep-start"
                    >
                      {busy === 'unprep-dry' ? 'Checking…' : 'Undo box setup…'}
                    </Button>
                  )}
                </div>
              </div>
            </SettingsGroup>
          ) : null}
        </>
      ) : null}

      <p className={`text-[11px] ${muted}`}>
        Which agents may build and deploy Apps is set per agent: Settings → Workspaces → the agent → Agent.
      </p>
    </div>
  )
}

function CheckRow({ check }: { check: BoxCheck }): React.JSX.Element {
  return (
    <li className="text-[11px]" data-testid={`apps-check-${check.id}`} data-status={check.status}>
      <span className={`inline-block w-12 uppercase tracking-wider text-[9px] font-semibold ${STATUS_STYLE[check.status]}`}>
        {check.status}
      </span>
      <span className="text-[var(--color-text-primary)]">{checkLabel(check.id)}</span>
      {check.detail ? <span className={muted}> · {check.detail}</span> : null}
      {check.fix && check.status !== 'ok' ? (
        <div className={`ml-12 ${secondary}`}>
          Fix: <span className="font-mono">{check.fix}</span>
        </div>
      ) : null}
    </li>
  )
}
