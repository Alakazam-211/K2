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
import { BetaTag, Button, Callout, OverflowMenu, Toggle } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import { useSettingsStore } from '@/stores/settings'
import { useConnectHostStore } from '@/stores/connect-host'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import {
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SETTING_SERVER_REASON,
  SettingRow,
  SettingSwitchRow,
  SettingsGroup,
  SettingsLoadState,
} from '../controls/SettingControls'
import { AccessSheet } from './AccessDialogs'
import { LinuxOnlyNote } from './LinuxOnlyNote'
import { PaneHeader, StatusDot, serverWords, type DotTone } from './apps-ui'
import type { SettingEntry } from '../searchManifest'
import {
  changeLines,
  checkLabel,
  undoWords,
  engineNeeded,
  appEngineLine,
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
    label: 'Apps on this server',
    beta: true,
    description: 'Turn Apps on or off, check and set up this server, and install the App runtime',
    keywords: ['apps', 'app', 'beta', 'this server', 'doctor', 'prepare', 'box', 'podman', 'engine', 'k2 app doctor', 'setup'],
    group: 'This server',
  },
]

type BoxState = { kind: 'loading' } | { kind: 'off' } | { kind: 'ready'; doctor: BoxDoctor } | { kind: 'error'; text: string }

type Notice = { tone: 'ok' | 'error' | 'info' | 'warn'; text: string }

/** `middle` renders between the Apps switch and the box checks: the page
 *  puts the list of Apps there, so what a person comes for comes first. */
export function AppsServerSection({ middle }: { middle?: React.ReactNode } = {}): React.JSX.Element {
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
  const [checking, setChecking] = useState(false)
  const [showChecks, setShowChecks] = useState(false)
  const mounted = useRef(true)

  const isOwner = role === 'owner'

  const load = useCallback(async (): Promise<void> => {
    try {
      const d = await fetchAppBox()
      if (mounted.current) setBox({ kind: 'ready', doctor: d })
    } catch (e) {
      if (!mounted.current) return
      setBox(isAppsOff(e) ? { kind: 'off' } : { kind: 'error', text: serverWords(daemonErrorText(e)) })
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

  const recheck = async (): Promise<void> => {
    setChecking(true)
    if (box.kind !== 'ready') setBox({ kind: 'loading' })
    await load()
    if (mounted.current) setChecking(false)
  }

  const flipApps = async (next: boolean): Promise<void> => {
    setSwitchBusy(true)
    setNotice(null)
    try {
      await setApps(next)
      setDry(null)
      await load()
    } catch (e) {
      setNotice({ tone: 'error', text: `Couldn’t turn Apps ${next ? 'on' : 'off'}: ${serverWords(daemonErrorText(e))}. Try again.` })
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
      setNotice({ tone: 'error', text: `Couldn’t work out what Prepare would change: ${serverWords(daemonErrorText(e))}. Try again.` })
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
      setNotice({ tone: 'ok', text: 'The server is prepared.' })
      await load()
    } catch (e) {
      setNotice({
        tone: 'error',
        text: `Couldn’t prepare the server: ${serverWords(daemonErrorText(e))}. The checks below show where it is now.`,
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
      setNotice({ tone: 'error', text: `Couldn’t change automatic setup: ${serverWords(daemonErrorText(e))}. Try again.` })
    } finally {
      setBusy(null)
    }
  }

  const retryAuto = async (): Promise<void> => {
    setBusy('retry')
    setNotice(null)
    try {
      await retryAutoPrepare()
      setNotice({ tone: 'info', text: 'Automatic setup will run again now.' })
      await load()
    } catch (e) {
      setNotice({ tone: 'error', text: `Couldn’t restart automatic setup: ${serverWords(daemonErrorText(e))}. Try again.` })
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
      setNotice({ tone: 'error', text: `Couldn’t work out what Undo would remove: ${serverWords(daemonErrorText(e))}. Try again.` })
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
        text: `The server setup is undone.${r.kept.length || r.note ? ' The Apps user, App data and K2’s container engine stay.' : ''} Automatic setup pauses until you press Retry.`,
      })
      await load()
    } catch (e) {
      setNotice({ tone: 'error', text: `Couldn’t prepare the server: ${serverWords(daemonErrorText(e))}. The checks below show where it is now.` })
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
      setNotice({ tone: 'ok', text: `The App runtime ${r.version ?? ''} is installed and answering.`.replace('  ', ' ') })
      await load()
    } catch (e) {
      setNotice({ tone: 'error', text: `Couldn’t install the App runtime: ${serverWords(daemonErrorText(e))}. Try again.` })
      await load()
    } finally {
      setBusy(null)
    }
  }

  const doctor = box.kind === 'ready' ? box.doctor : null
  const appsOn = box.kind === 'off' ? false : doctor ? doctor.apps : apps
  const ownerOnly = !isOwner && role !== null
  const attention = doctor ? doctor.checks.filter((c) => c.status !== 'ok').length : 0
  const okChecks = doctor ? doctor.checks.filter((c) => c.status === 'ok') : []
  const engineLine = doctor ? appEngineLine(doctor.appEngine) : null

  return (
    <div className="space-y-6" data-testid="apps-server" data-settings-id="skin-access.server">
      <PaneHeader
        title="This server"
        hint="Apps your agents build run here, each walled off in its own container. Set the server up from here; no terminal needed."
      />

      {doctor && !doctor.linux ? (
        <LinuxOnlyNote testId="apps-linux-note">
          Apps run on Linux servers. On this Mac, agents can build and check Apps but not run them.
        </LinuxOnlyNote>
      ) : null}

      {notice ? (
        <Callout tone={notice.tone} className="type-hint" role={notice.tone === 'error' ? 'alert' : 'status'} data-testid="apps-server-notice">
          {notice.text}
        </Callout>
      ) : null}

      <SettingsGroup card title="Apps">
        <SettingSwitchRow
          control={
            <Toggle checked={appsOn} onChange={(v) => void flipApps(v)} disabled={!isOwner || switchBusy} aria-label="Apps" />
          }
        >
          <div className={`${SETTING_ROW_LABEL} flex items-center gap-2`}>
            Apps
            <BetaTag />
          </div>
          <p className={SETTING_ROW_HINT}>
            Let the agents you allow build, install and run Apps on this server. Off closes every App.
          </p>
          {ownerOnly ? (
            <p className={SETTING_ROW_HINT} data-testid="apps-switch-owner-only">
              Only the server&rsquo;s owner can turn Apps on or off.
            </p>
          ) : null}
        </SettingSwitchRow>
        <SettingRow label="Which agents may build Apps" hint="Set on each agent’s card, under Permissions.">
          <Button onClick={() => useSettingsStore.getState().openSettings('projects')} data-testid="apps-open-agents">
            Open Workspaces / Agents
          </Button>
        </SettingRow>
      </SettingsGroup>

      {middle}

      <SettingsGroup
        card
        title="Server checks"
        hint="What this server needs to run Apps safely."
        action={
          doctor ? (
            <Button onClick={() => void recheck()} disabled={checking} data-testid="apps-check-again">
              {checking ? 'Checking…' : 'Check again'}
            </Button>
          ) : undefined
        }
        state={
          box.kind === 'loading' ? (
            <SettingsLoadState kind="loading" title="Checking this server…" />
          ) : box.kind === 'error' ? (
            <SettingsLoadState
              kind="error"
              title="K2 couldn’t check this server"
              detail={box.text}
              action={<Button onClick={() => void recheck()}>Try again</Button>}
            >
              The checks didn&rsquo;t load. Nothing on the server changed.
            </SettingsLoadState>
          ) : box.kind === 'off' ? (
            <SettingsLoadState kind="empty" title="Apps are off" testId="apps-off">
              Turn on Apps above to check and set up this server.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {doctor ? (
          <div data-testid="apps-doctor" className="divide-y divide-[var(--color-border)]">
            <div className="px-4 py-3 flex items-start gap-3">
              <StatusDot tone={doctor.ready ? 'ok' : 'warn'} className="mt-1.5" />
              <div className="min-w-0 flex-1">
                <div className="type-name text-[var(--color-text-primary)]" data-testid="apps-checks-summary">
                  {checksSummary(doctor)}
                </div>
                <p className={SETTING_ROW_HINT}>
                  {!doctor.linux
                    ? 'Apps run walled on Linux servers. This machine can build and check Apps, not run them.'
                    : doctor.ready
                      ? attention
                        ? 'Ready to run Apps. The check below is worth a look.'
                        : 'Ready to run Apps.'
                      : 'The checks below need attention. Prepare this server, under Set up, fixes most of them.'}
                </p>
              </div>
              {okChecks.length > 0 ? (
                <Button
                  aria-expanded={showChecks}
                  aria-controls="apps-checks-passing"
                  onClick={() => setShowChecks((v) => !v)}
                  data-testid="apps-checks-toggle"
                >
                  {showChecks ? 'Hide details' : 'Show details'}
                </Button>
              ) : null}
            </div>
            {doctor.checks
              .filter((c) => c.status !== 'ok')
              .map((c) => (
                <CheckRow key={c.id} check={c} />
              ))}
            {showChecks ? (
              <div id="apps-checks-passing" className="divide-y divide-[var(--color-border)]">
                {okChecks.map((c) => (
                  <CheckRow key={c.id} check={c} />
                ))}
              </div>
            ) : null}
          </div>
        ) : null}
      </SettingsGroup>

      {doctor && doctor.linux ? (
        <SettingsGroup
          card
          title="Set up this server"
          hint="K2 makes these changes through its admin helper. You see each change first."
          action={
            <OverflowMenu
              label="More set-up actions"
              triggerProps={{ 'data-testid': 'apps-setup-menu' }}
              items={[
                {
                  id: 'unprep',
                  label: busy === 'unprep-dry' ? 'Checking…' : 'Undo the setup…',
                  hint: isOwner ? 'Removes what Prepare set up. Not while an App runs.' : 'Only the server’s owner can undo the setup',
                  danger: true,
                  disabled: !isOwner || busy !== null,
                  onSelect: () => void unprepPlan(),
                },
              ]}
            />
          }
        >
          <div data-testid="apps-setup" className="divide-y divide-[var(--color-border)]">
            {ownerOnly ? (
              <div className="px-4 py-3">
                <p className={SETTING_ROW_HINT}>Only the server&rsquo;s owner can set up the server or install the App runtime.</p>
              </div>
            ) : null}
            {doctor.autoPrepare ? (
              <SettingSwitchRow
                testId="apps-auto-prepare"
                control={
                  <Toggle
                    checked={doctor.autoPrepare.enabled}
                    onChange={(v) => void flipAuto(v)}
                    disabled={!isOwner || busy !== null}
                    aria-label="Prepare this box automatically"
                  />
                }
              >
                <div className={SETTING_ROW_LABEL}>Keep this server set up</div>
                <p className={SETTING_ROW_HINT}>
                  After each K2 update, K2 sets the server up for Apps by itself, so agents you allow can deploy without
                  anyone typing commands on it. It stops and tells you if the server already runs containers another way.
                </p>
                {doctor.autoPrepare.state ? (
                  <p className={SETTING_ROW_HINT} data-testid="apps-auto-prepare-last">
                    Last run: {LAST_RUN_WORDS[doctor.autoPrepare.state] ?? doctor.autoPrepare.state}
                    {doctor.autoPrepare.detail ? ` (${doctor.autoPrepare.detail})` : '.'}
                  </p>
                ) : null}
                {doctor.autoPrepare.undoneAt !== null || doctor.autoPrepare.blocked ? (
                  <div className="mt-2 flex items-start justify-between gap-4" data-testid="apps-auto-prepare-stopped">
                    <p className="type-hint text-[var(--color-status-warn-text)] leading-relaxed">
                      {doctor.autoPrepare.undoneAt !== null
                        ? 'Paused because you undid the setup.'
                        : `Stopped: ${doctor.autoPrepare.detail ?? doctor.autoPrepare.code ?? 'this server can’t be set up'}.`}{' '}
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
              </SettingSwitchRow>
            ) : null}
            <SettingRow
              label="Prepare this server"
              hint={`Sets up the Apps user, its limits, the firewall and the container runtime. You see every change before anything happens.${prepareNeeded(doctor) ? '' : ' The checks say it’s set up; running it again is safe.'}`}
            >
              <Button
                variant={prepareNeeded(doctor) ? 'accent' : 'default'}
                onClick={() => void runDry()}
                disabled={busy !== null}
                data-testid="apps-prepare-dry-run"
              >
                {busy === 'dry' ? 'Checking…' : 'Prepare…'}
              </Button>
            </SettingRow>
            <SettingRow
              label="App runtime"
              hint={`Runs each App in its own container. Signed by K2 and matched to this server’s K2.${engineNeeded(doctor) ? '' : ' It’s installed and answering.'}`}
              below={
                engineLine ? (
                  <p className="type-hint text-[var(--color-status-error-soft)] leading-relaxed" data-testid="apps-engine-status">
                    {engineLine}
                  </p>
                ) : undefined
              }
            >
              <Button
                variant={engineNeeded(doctor) ? 'accent' : 'default'}
                onClick={() => void engine()}
                disabled={!isOwner || busy !== null}
                data-testid="apps-engine-install"
              >
                {busy === 'engine' ? 'Installing…' : engineNeeded(doctor) ? 'Install App runtime' : 'Reinstall App runtime'}
              </Button>
            </SettingRow>
          </div>
        </SettingsGroup>
      ) : null}

      {dry ? (
        <AccessSheet
          title="Prepare this server?"
          intro="Prepare makes these changes, in this order. Nothing has changed yet."
          onClose={() => setDry(null)}
          busy={busy === 'apply'}
          testId="apps-dry-run"
          footer={
            <>
              <Button variant="ghost" size="md" onClick={() => setDry(null)} disabled={busy !== null} autoFocus>
                Cancel
              </Button>
              <Button
                variant="accent"
                size="md"
                onClick={() => void apply()}
                disabled={!isOwner || busy !== null}
                data-testid="apps-prepare-apply"
              >
                {busy === 'apply' ? 'Preparing… (a few minutes)' : needsPodmanInstall(dry) ? 'Prepare (installs the container engine)' : 'Prepare'}
              </Button>
            </>
          }
        >
          <ol className="list-decimal ml-5 space-y-1">
            {changeLines(dry).map((l, i) => (
              <li key={i} className="type-hint text-[var(--color-text-primary)] break-words leading-relaxed">
                {l}
              </li>
            ))}
          </ol>
          {needsPodmanInstall(dry) && dry.podman.bundle ? (
            <p className={`${SETTING_SERVER_REASON} break-all`}>
              Container engine: Podman {dry.podman.bundle.version} from {dry.podman.bundle.url} (sha256 {dry.podman.bundle.sha256})
            </p>
          ) : null}
          {!isOwner ? <p className={SETTING_ROW_HINT}>Only the server&rsquo;s owner can prepare it.</p> : null}
        </AccessSheet>
      ) : null}

      {unprep ? (
        <AccessSheet
          title="Undo the server setup?"
          intro="Apps stop working on this server until it’s prepared again. Automatic setup pauses until you press Retry."
          onClose={() => setUnprep(null)}
          busy={busy === 'unprep'}
          testId="apps-unprep-plan"
          footer={
            <>
              <Button variant="ghost" size="md" onClick={() => setUnprep(null)} disabled={busy !== null} autoFocus>
                Cancel
              </Button>
              <Button
                variant="danger-muted"
                size="md"
                onClick={() => void unprepConfirm()}
                disabled={!isOwner || busy !== null || unprep.runningApps.length > 0}
                data-testid="apps-unprep-confirm"
              >
                {busy === 'unprep' ? 'Undoing…' : 'Undo setup'}
              </Button>
            </>
          }
        >
          <div>
            <div className={SETTING_ROW_LABEL}>It removes</div>
            <ul className="list-disc ml-5 mt-1 space-y-0.5">
              {unprep.undoes.map((l, i) => (
                <li key={i} className="type-hint text-[var(--color-text-primary)] break-words" title={l}>
                  {undoWords(l)}
                </li>
              ))}
            </ul>
          </div>
          {unprep.kept.length ? <p className={SETTING_ROW_HINT}>It keeps: {unprep.kept.map(undoWords).join('; ')}.</p> : null}
          {unprep.runningApps.length ? (
            <Callout tone="warn" className="type-hint" data-testid="apps-unprep-running">
              Stop these Apps first: {unprep.runningApps.join(', ')}
            </Callout>
          ) : null}
        </AccessSheet>
      ) : null}
    </div>
  )
}

const LAST_RUN_WORDS: Record<string, string> = {
  ready: 'nothing needed changing',
  prepared: 'set things up',
  skipped: 'skipped',
  failed: 'failed',
  blocked: 'stopped',
}

/** A check's value in plain words: no K2-internal notes, just the number
 *  a person reads ("Podman 5.2", "38 GB"); the kernel's own version is left
 *  out. Unknown checks show the daemon's detail. */
export function checkValue(c: Pick<BoxCheck, 'id' | 'detail'>): string {
  const d = serverWords(c.detail.trim())
  if (!d) return ''
  switch (c.id) {
    case 'kernel':
      return ''
    case 'podman': {
      const v = d.replace(/\s*\(K2[’']s\)\s*$/i, '')
      return /^\d/.test(v) ? `Podman ${v}` : v.replace(/^podman\b/i, 'Podman')
    }
    case 'disk': {
      const m = /^([\d.,]+\s*[KMGT]?B)\b/i.exec(d)
      return m ? m[1] : d
    }
    default:
      return d
  }
}

/** The one line that sums the checks up. */
function checksSummary(d: BoxDoctor): string {
  const total = d.checks.length
  const ok = d.checks.filter((c) => c.status === 'ok').length
  if (!d.ready) return `Not ready yet: ${total - ok} of ${total} checks need attention`
  return ok === total ? `Ready for Apps: all ${total} checks pass` : `Ready for Apps: ${ok} of ${total} checks pass`
}

const CHECK_WORDS: Record<string, string> = { ok: 'OK', warn: 'Check', fail: 'Failed', unknown: 'Unknown' }
const CHECK_DOT: Record<string, DotTone> = { ok: 'ok', warn: 'warn', fail: 'error', unknown: 'idle' }

/** Where a check's fix lives: the button on this page when there is one,
 *  else the daemon's own words for the server. */
export function fixWords(fix: string): string {
  if (fix.startsWith('k2 app box prepare')) return 'Fix: Prepare this server, under Set up.'
  if (fix.startsWith('k2 app engine')) return 'Fix: Install App runtime, under Set up.'
  return `Fix on the server: ${fix}`
}

function CheckRow({ check }: { check: BoxCheck }): React.JSX.Element {
  return (
    <div className="px-4 py-2.5 flex items-start gap-3" data-testid={`apps-check-${check.id}`} data-status={check.status}>
      <StatusDot tone={CHECK_DOT[check.status] ?? 'idle'} className="mt-1.5" />
      <div className="min-w-0 flex-1">
        <span className="type-row text-[var(--color-text-primary)]">
          {checkLabel(check.id)}
          {checkValue(check) ? `: ${checkValue(check)}` : ''}
        </span>
        {check.fix && check.status !== 'ok' ? <p className={SETTING_SERVER_REASON}>{fixWords(check.fix)}</p> : null}
      </div>
      <span className={`type-hint flex-shrink-0 ${check.status === 'ok' ? 'text-[var(--color-text-secondary)]' : check.status === 'fail' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-status-warn-text)]'}`}>
        {CHECK_WORDS[check.status] ?? check.status}
      </span>
    </div>
  )
}
