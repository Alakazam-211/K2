// "Get this server ready for Apps" (0.46.16, Rosson: K2 Apps should be ready
// on every server). On a server someone installed or upgraded themselves, the
// daemon offers its owner one card when the box isn't ready for Apps and
// something happened (an upgrade, a paid plan starting, the owner opening
// the Apps page). One click runs the same setup as `k2 app box prepare`
// through K2's root helper, with progress and the doctor's answer at the end.
// It never restarts K2 by itself: when the agents' memory budget needs one,
// the owner picks "Restart now" or "Tonight at 3 am" (their own time), and
// the card says whether any guest would be signed out.
//
// Thin client: the daemon decides when to offer it (once per trigger), that
// only owners see it, and does all of the work; this shows its answers. It
// reads on start, on window focus, when the Apps page opens and when opened
// from Settings, and only
// while a run is going does it read every two seconds.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui'
import { PRODUCT_NAME } from '@/lib/brand'
import { daemonErrorText } from '@/lib/daemon-error'
import { useConnectHostStore } from '@/stores/connect-host'
import { AccessSheet, DIALOG_MUTED } from '@/components/Settings/sections/AccessDialogs'
import {
  APPS_PAGE_EVENT,
  OPEN_EVENT,
  cancelRestart,
  dismissOffer,
  fetchReady,
  guestsText,
  noteAppsPageOpened,
  phaseText,
  restartNow,
  restartTonight,
  startRun,
  whenText,
  type ReadyStatus,
} from './apps-ready-api'

export const POLL_MS = 2000
const TITLE = 'Get this server ready for Apps'
const LINE =
  'Installs Podman and system packages, sets up the App user and firewall. No restart now: if one is needed, you pick when.'

export function AppsReadyOffer(): React.JSX.Element | null {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const [status, setStatus] = useState<ReadyStatus | null>(null)
  /** Opened by hand (Settings), or kept open to watch a run this window started. */
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState<null | 'run' | 'tonight' | 'now' | 'cancel'>(null)
  const [error, setError] = useState<string | null>(null)
  const [restarting, setRestarting] = useState(false)
  const mounted = useRef(true)

  const read = useCallback(async (): Promise<void> => {
    try {
      const s = await fetchReady()
      if (mounted.current) setStatus(s)
    } catch (e) {
      // Apps off, an older server, a non-owner: no card.
      console.debug('[apps-ready] read:', e)
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    setStatus(null)
    setOpen(false)
    void read()
    const onFocus = (): void => void read()
    const onOpen = (): void => {
      setError(null)
      setOpen(true)
      void read()
    }
    const onAppsPage = (): void => {
      noteAppsPageOpened()
        .then((s) => {
          if (mounted.current) setStatus(s)
        })
        .catch((e: unknown) => console.debug('[apps-ready] apps page:', e))
    }
    window.addEventListener('focus', onFocus)
    window.addEventListener(OPEN_EVENT, onOpen)
    window.addEventListener(APPS_PAGE_EVENT, onAppsPage)
    return () => {
      mounted.current = false
      window.removeEventListener('focus', onFocus)
      window.removeEventListener(OPEN_EVENT, onOpen)
      window.removeEventListener(APPS_PAGE_EVENT, onAppsPage)
    }
  }, [read, activeHost])

  const running = status?.run?.state === 'running'
  useEffect(() => {
    if (!running) return
    const t = window.setInterval(() => void read(), POLL_MS)
    return () => window.clearInterval(t)
  }, [running, read])

  const act = async (what: NonNullable<typeof busy>, f: () => Promise<ReadyStatus | void>): Promise<void> => {
    setBusy(what)
    setError(null)
    try {
      const s = await f()
      if (s && mounted.current) setStatus(s)
    } catch (e) {
      if (mounted.current) setError(daemonErrorText(e))
    } finally {
      if (mounted.current) setBusy(null)
    }
  }

  if (!status || !status.owner) return null
  const offer = status.offer
  if (!open && !offer) return null

  const close = (): void => {
    setOpen(false)
    // Closing the card answers a waiting offer: it isn't offered again for that trigger.
    if (offer) {
      setStatus({ ...status, offer: null })
      void dismissOffer(offer.trigger).catch((e: unknown) => console.debug('[apps-ready] dismiss:', e))
    }
  }

  const run = status.run
  const offerView = !!offer && !open
  const restartNeeded = status.restartNeeded || (run?.state === 'done' && run.restartNeeded)
  const canStart = offerView || !run || run.state === 'failed' || (run.state === 'done' && run.ready === false)
  const begin = (): void =>
    void act('run', async () => {
      setOpen(true)
      return startRun()
    })
  const startLabel = busy === 'run' ? 'Starting…' : run && !offerView ? 'Run it again' : 'Get ready'

  const footer = running ? (
    <Button variant="ghost" size="md" onClick={() => setOpen(false)}>
      Hide
    </Button>
  ) : (
    <>
      <Button variant="ghost" size="md" onClick={close} disabled={busy !== null}>
        {offerView ? 'Not now' : 'Close'}
      </Button>
      {canStart ? (
        <Button variant="accent" size="md" autoFocus disabled={busy !== null || !status.linux} onClick={begin}>
          {startLabel}
        </Button>
      ) : null}
    </>
  )

  return (
    <AccessSheet title={offer?.title || TITLE} onClose={running ? () => setOpen(false) : close} busy={busy !== null} testId="apps-ready-offer" footer={footer}>
      <div className="space-y-3">
        {offerView || !run ? (
          <p className={DIALOG_MUTED} data-testid="apps-ready-line">
            {offer?.line || LINE}
          </p>
        ) : null}
        {!status.linux ? <p className={DIALOG_MUTED}>Apps run on Linux servers. This server can’t run them.</p> : null}
        {!offerView && run && running ? (
          <p className={DIALOG_MUTED} role="status" data-testid="apps-ready-progress">
            {phaseText(run.phase)}
          </p>
        ) : null}
        {!offerView && run?.state === 'done' ? (
          run.ready ? (
            <p className={DIALOG_MUTED} role="status" data-testid="apps-ready-done">
              This server is ready for Apps.
            </p>
          ) : (
            <BlockingList title="The setup finished, but the server isn’t ready yet:" rows={run.blocking} />
          )
        ) : null}
        {!offerView && run?.state === 'failed' ? (
          <div data-testid="apps-ready-failed">
            <p className={DIALOG_MUTED} role="alert">
              The setup stopped: {run.error ?? 'unknown error'}. Nothing was restarted. Run it again, or use k2 app box prepare.
            </p>
            {run.blocking.length ? <BlockingList title="Still to fix:" rows={run.blocking} /> : null}
          </div>
        ) : null}
        {!offerView && restartNeeded && !running ? (
          <div className="space-y-2" data-testid="apps-ready-restart">
            <p className={DIALOG_MUTED} data-testid="apps-ready-guests">
              One short {PRODUCT_NAME} restart finishes it: agent sessions get their own memory budget. {guestsText(status.guests)}
            </p>
            {status.restart ? (
              <p className={DIALOG_MUTED} data-testid="apps-ready-scheduled">
                Restart scheduled for {whenText(status.restart.at)} ({status.restart.zone}).
              </p>
            ) : null}
            {restarting ? (
              <p className={DIALOG_MUTED} role="status">
                {PRODUCT_NAME} is restarting. This window reconnects by itself.
              </p>
            ) : (
              <div className="flex flex-wrap gap-2">
                {status.restart ? (
                  <Button size="md" disabled={busy !== null} onClick={() => void act('cancel', cancelRestart)}>
                    Cancel the restart
                  </Button>
                ) : (
                  <Button size="md" disabled={busy !== null} onClick={() => void act('tonight', restartTonight)}>
                    Tonight at 3 am
                  </Button>
                )}
                <Button
                  variant="accent"
                  size="md"
                  disabled={busy !== null}
                  onClick={() =>
                    void act('now', async () => {
                      await restartNow()
                      setRestarting(true)
                    })
                  }
                >
                  Restart now
                </Button>
              </div>
            )}
          </div>
        ) : null}
        {error ? (
          <p className={DIALOG_MUTED} role="alert" data-testid="apps-ready-error">
            {error}
          </p>
        ) : null}
      </div>
    </AccessSheet>
  )
}

function BlockingList({ title, rows }: { title: string; rows: ReadyStatus['blocking'] }): React.JSX.Element {
  return (
    <div data-testid="apps-ready-blocking">
      <p className={DIALOG_MUTED}>{title}</p>
      <ul className="list-disc pl-5">
        {rows.map((r) => (
          <li key={r.id} className={DIALOG_MUTED}>
            {r.detail || r.id}
            {r.fix ? ` (fix: ${r.fix})` : ''}
          </li>
        ))}
      </ul>
    </div>
  )
}

export default AppsReadyOffer
