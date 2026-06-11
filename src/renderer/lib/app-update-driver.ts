// Shape A driver (0.39.35 — unified remote update).
//
// When THIS machine is a "bundled-app" host (the daemon runs inside
// K2.app), a remote Owner/Admin can trigger an update of it over K2
// Connect. The daemon receives `POST /cli/daemon/update/start`, sees it's a
// bundled-app, and EMITS an `app:update-trigger` frame over its `/events`
// WS. The co-located app's `daemon_events` subscriber re-emits that as a
// Tauri event; this module listens for it and DRIVES the app's OWN Tauri
// updater (the only thing that can swap a signed/notarized .app), POSTing
// each phase back to the LOCAL daemon's `/cli/daemon/app-update/progress`
// so the remote's `/cli/daemon/update/status` poll surfaces progress
// uniformly.
//
// Why a dedicated LOCAL post (not `daemonCliPost`): the trigger fires on
// THIS host's own daemon, and the phase relay must always go back to THAT
// daemon — independent of whatever remote host the user happens to be
// VIEWING in this app's `activeHost`. So we resolve the LOCAL daemon creds
// directly via the Tauri `daemon_ws_url` command and POST to loopback.

import { invoke } from '@tauri-apps/api/core'
import { useUpdateStore } from '@/stores/update'
import type { UpdatePhase } from '@/components/Settings/sections/update-host'

/** The Tauri event the daemon's `app:update-trigger` WireEvent surfaces as
 *  (re-emitted verbatim by `src-tauri/src/daemon_events.rs`). */
export const APP_UPDATE_TRIGGER_EVENT = 'app:update-trigger'

interface TriggerPayload {
  job_id?: string
}

interface LocalDaemonCreds {
  port: number
  token: string
}

interface RawDaemonWsUrl {
  state: 'available' | 'not_installed'
  port?: number
  token?: string
  reason?: string
}

/** Resolve the LOCAL daemon's loopback creds (port + token) via the Tauri
 *  command. Throws if the local daemon isn't reachable. */
async function resolveLocalDaemon(): Promise<LocalDaemonCreds> {
  const res = await invoke<RawDaemonWsUrl>('daemon_ws_url')
  if (res.state !== 'available' || !res.port || !res.token) {
    throw new Error(`local daemon not reachable: ${res.reason ?? 'unknown'}`)
  }
  return { port: res.port, token: res.token }
}

/** POST one phase update back to the LOCAL daemon so the remote's
 *  update/status poll reflects it. Best-effort: a relay failure must never
 *  abort the actual update, so callers swallow errors. */
export async function postPhase(
  creds: LocalDaemonCreds,
  jobId: string,
  phase: UpdatePhase,
  extra?: { progress?: number; error?: string },
): Promise<void> {
  const url = `http://127.0.0.1:${creds.port}/cli/daemon/app-update/progress?token=${creds.token}`
  await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      job_id: jobId,
      phase,
      ...(typeof extra?.progress === 'number' ? { progress: extra.progress } : {}),
      ...(extra?.error ? { error: extra.error } : {}),
    }),
  })
}

/** Run the Tauri-updater sequence for a triggered Shape A update, relaying
 *  each phase back to the local daemon. Exported for direct testing /
 *  re-use; the listener (below) just forwards the job_id here. */
export async function runTriggeredAppUpdate(jobId: string): Promise<void> {
  let creds: LocalDaemonCreds
  try {
    creds = await resolveLocalDaemon()
  } catch (e) {
    // Can't even reach the local daemon — nothing to relay TO. Bail; the
    // remote will see the job stay where it was and time out its poll.
    console.error('[app-update] local daemon unreachable:', e)
    return
  }

  const store = useUpdateStore.getState()
  const relayError = async (msg: string): Promise<void> => {
    try {
      await postPhase(creds, jobId, 'failed', { error: msg })
    } catch {
      /* relay is best-effort */
    }
  }

  try {
    // 1. Check + (if available) download. The store's checkForUpdate wraps
    //    the plugin's `check()`; startDownload wraps downloadAndInstall.
    await postPhase(creds, jobId, 'downloading').catch(() => {})
    const available = await store.checkForUpdate()
    if (!available) {
      // Nothing to install — report it as a (benign) failure so the remote
      // doesn't wait forever. The host stays on its current version.
      await relayError('No update available for this app.')
      return
    }
    await store.startDownload()
    const afterDownload = useUpdateStore.getState()
    if (afterDownload.status === 'error') {
      await relayError(afterDownload.error ?? 'Download failed.')
      return
    }

    // 2. Download finished + staged → about to install + relaunch. There's
    //    no separate manual "apply" for Shape A: the app auto-installs +
    //    relaunches, so we drive straight through applying → restarting.
    await postPhase(creds, jobId, 'applying').catch(() => {})
    await postPhase(creds, jobId, 'restarting').catch(() => {})

    // 3. Install + relaunch. This terminates the app, so any code after
    //    installAndRelaunch only runs if relaunch FAILED.
    await store.installAndRelaunch()
    const afterInstall = useUpdateStore.getState()
    if (afterInstall.status === 'error') {
      await relayError(afterInstall.error ?? 'Install/relaunch failed.')
    }
  } catch (e) {
    await relayError(e instanceof Error ? e.message : String(e))
  }
}

/** Handle a raw trigger payload: validate the job_id, then drive the
 *  update. Exported so the hook + tests share one entry point. */
export async function handleAppUpdateTrigger(payload: unknown): Promise<void> {
  const jobId = (payload as TriggerPayload | null)?.job_id
  if (typeof jobId !== 'string' || jobId.length === 0) {
    console.error('[app-update] trigger missing job_id:', payload)
    return
  }
  await runTriggeredAppUpdate(jobId)
}
