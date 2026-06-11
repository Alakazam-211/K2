// K2 Connect — "Clone to" orchestration (renderer half).
//
// PRD: .k2so/prds/k2-connect-clone-to.md (P2 — Clone to / high bar).
//
// The backend is DONE on main:
//   - daemon `POST /cli/clone/bundle  { project_path } -> { bundle_path, manifest_summary }`
//   - daemon `POST /cli/clone/unpack  { bundle_path, dest_parent } -> { project, dest_path }`
//   - daemon `POST /cli/fs/upload-binary { dir, filename, base64 } -> { path }`
//   - Tauri  `read_local_file_base64(path) -> base64`
//   - daemon `GET  /cli/fs/info -> { home, separator, os }`  (host fs probe)
//
// This module sequences those calls so the user can clone a LOCAL workspace
// onto a connected K2 Connect host. The ORDER is load-bearing: each daemon
// call targets the THEN-ACTIVE host (daemonCliPost reads `activeHost` at
// call time), so we build + read the bundle while LOCAL is active, THEN
// switch hosts, THEN browse + upload + unpack against the destination. No
// host-targeting plumbing is needed — only correct sequencing.
//
// `cloneWorkspaceTo` takes all of its side-effecting collaborators as
// injected deps (a `CloneDeps` bag) so the sequencing is unit-testable with
// mocks. The default wiring (`defaultCloneDeps`) binds the real stores /
// daemon-cli / invoke. The progress modal owns the `hooks` (stage updates +
// summary surfacing); the orchestration is otherwise UI-free.

import type { ConnectHost } from '@/stores/connect-host'

/** Manifest summary the daemon returns from `clone/bundle`. */
export interface CloneManifestSummary {
  entry_count: number
  scrubbed_secret_count: number
  size_bytes: number
  /** Optional: scrubbed secrets by relative path, if the backend surfaces
   *  them. Absent today (the summary only carries the COUNT) — see the
   *  re-supply note in the done screen. Kept optional so we light it up
   *  for free when the backend starts returning it. */
  scrubbed_secrets?: string[]
}

/** `clone/bundle` response. */
export interface CloneBundleResult {
  bundle_path: string
  manifest_summary: CloneManifestSummary
}

/** Minimal project shape the unpack route returns (the freshly-registered
 *  remote project). We don't model the whole row — the done screen only
 *  needs the path. */
export interface ClonedProject {
  id?: string
  name?: string
  path?: string
  [k: string]: unknown
}

/** `clone/unpack` response. */
export interface CloneUnpackResult {
  project: ClonedProject
  dest_path: string
}

/** Host filesystem info (`fs/info`). */
export interface CloneFsInfo {
  home: string
  separator: string
  os: string
}

/** The stages the progress UI moves through. Kept in this module so the
 *  modal + orchestration share one vocabulary. */
export type CloneStage =
  | 'bundling'
  | 'connecting'
  | 'choosing-folder'
  | 'uploading'
  | 'unpacking'
  | 'done'
  | 'error'

/** UI callbacks. All optional so a headless caller (tests) can omit them. */
export interface CloneHooks {
  /** Called as the flow advances. */
  onStage?: (stage: CloneStage) => void
  /** Called once the bundle is built, with its manifest summary. */
  onBundled?: (summary: CloneManifestSummary) => void
  /** Called on success with the unpack result. */
  onDone?: (result: CloneUnpackResult) => void
  /** Called on any failure with a human-readable message. */
  onError?: (message: string) => void
}

/**
 * Side-effecting collaborators, injected so the sequencing is testable.
 * The defaults (`defaultCloneDeps`) bind the real implementations.
 */
export interface CloneDeps {
  /** POST a `/cli/*` route against the ACTIVE host. */
  daemonCliPost: <T = unknown>(route: string, body?: unknown) => Promise<T>
  /** GET a `/cli/*` route against the ACTIVE host. */
  daemonCliGet: <T = unknown>(
    route: string,
    params?: Record<string, string | number | boolean | undefined | null>,
  ) => Promise<T>
  /** Read a LOCAL file's bytes as base64 (Tauri command). */
  readLocalFileBase64: (path: string) => Promise<string>
  /** Switch + authenticate to a host (connect-host store `pickHost`). */
  pickHost: (host: ConnectHost) => void
  /**
   * Wait until `host` is the active, connected host. Resolves when the
   * switch+auth completes; rejects if the user cancels sign-in or it times
   * out. Implemented over the connect-host store subscription.
   */
  waitForHostConnected: (host: ConnectHost, timeoutMs?: number) => Promise<void>
  /** Open the remote folder picker; resolves with the chosen REMOTE parent
   *  dir, or null on cancel. */
  pickRemoteFolder: () => Promise<string | null>
}

/** Last path segment of a path, tolerating BOTH `/` and `\` separators. */
export function basename(p: string): string {
  const seg = p.split(/[/\\]/).pop()
  return seg && seg.length > 0 ? seg : p
}

/** Raised when the user cancels a step (sign-in or folder pick). Carries a
 *  flag so callers can treat cancellation as a non-error abort if desired;
 *  the orchestration surfaces it through `onError` like any other failure
 *  message, but never proceeds to upload/unpack. */
export class CloneCancelledError extends Error {
  readonly cancelled = true
  constructor(message = 'Clone cancelled.') {
    super(message)
    this.name = 'CloneCancelledError'
  }
}

/**
 * Orchestrate "Clone to": build a scrubbed bundle of the LOCAL workspace at
 * `projectPath`, switch+auth to `destinationHost`, pick a parent folder
 * there, upload the bundle, and unpack it remotely.
 *
 * PRECONDITION (enforced by the caller, not here): only invoked when the
 * active host is 'local' — the bundle/read steps assume local is active.
 *
 * Step order (each targets the THEN-active host):
 *   1. [local]   clone/bundle  → { bundle_path, manifest_summary }
 *   1b.[local]   read_local_file_base64(bundle_path) → base64
 *   2. [switch]  pickHost(dest) + waitForHostConnected(dest)
 *   3. [dest]    pickRemoteFolder() → dest_parent  (cancel → abort)
 *   3b.[dest]    fs/info → host home, to derive a temp upload dir
 *   4. [dest]    fs/upload-binary { dir, filename, base64 } → remoteBundlePath
 *   5. [dest]    clone/unpack { bundle_path: remoteBundlePath, dest_parent }
 *                  → { project, dest_path }
 *
 * Returns the unpack result on success. Throws on any failure (including
 * cancellation, as a {@link CloneCancelledError}); the error is also
 * surfaced through `hooks.onError`.
 */
export async function cloneWorkspaceTo(
  projectPath: string,
  destinationHost: ConnectHost,
  deps: CloneDeps,
  hooks: CloneHooks = {},
  /**
   * Whether to carry secrets (.env, .auth/, in-workspace tokens) into the
   * bundle. Default `true` (include): the bundle travels over the encrypted
   * K2 Connect link between the user's own machines. When `false`, the
   * daemon scrubs secrets and the user re-supplies them on the host (the
   * done screen's re-supply checklist). Threaded into the `clone/bundle`
   * call as `carry_secrets`.
   */
  carrySecrets = true,
  /**
   * Whether to carry ALL Claude Code chat history (every session `.jsonl`),
   * not just the newest-mtime live one. Default `true` (include): Clone-to
   * is a true workspace-migration tool, so the full transcript history
   * travels by default (GitHub #21). When `false`, the daemon slims the
   * bundle down to just the live session. Threaded into the `clone/bundle`
   * call as `live_only` (the daemon's inverse-of-this flag).
   */
  includeAllHistory = true,
): Promise<CloneUnpackResult> {
  const { onStage, onBundled, onDone, onError } = hooks
  try {
    // ── 1. Build the bundle on the SOURCE (local is active) ──────────────
    onStage?.('bundling')
    const bundle = await deps.daemonCliPost<CloneBundleResult>('clone/bundle', {
      project_path: projectPath,
      carry_secrets: carrySecrets,
      live_only: !includeAllHistory,
    })
    if (!bundle?.bundle_path) {
      throw new Error('Bundle step returned no bundle_path.')
    }
    onBundled?.(bundle.manifest_summary)

    // 1b. Read the bundle bytes WHILE LOCAL IS STILL ACTIVE — it's a local
    //     file, and the read must happen before the host switch.
    const base64 = await deps.readLocalFileBase64(bundle.bundle_path)

    // ── 2. Switch + authenticate to the destination ─────────────────────
    onStage?.('connecting')
    deps.pickHost(destinationHost)
    await deps.waitForHostConnected(destinationHost)

    // ── 3. Pick the parent folder on the destination ────────────────────
    onStage?.('choosing-folder')
    const destParent = await deps.pickRemoteFolder()
    if (destParent === null) {
      throw new CloneCancelledError('Clone cancelled — no destination folder chosen.')
    }

    // 3b. Pick a temp dir on the host for the bundle upload. Prefer
    //     `~/.k2so/clone-tmp` (via fs/info home); fall back to dest_parent
    //     if fs/info isn't available, so the upload still lands somewhere
    //     writable on the host.
    let uploadDir = destParent
    try {
      const info = await deps.daemonCliGet<CloneFsInfo>('fs/info')
      if (info?.home) {
        const sep = info.separator || '/'
        const home = info.home.endsWith(sep) ? info.home.slice(0, -sep.length) : info.home
        uploadDir = `${home}${sep}.k2so${sep}clone-tmp`
      }
    } catch {
      // fs/info unavailable (old host) — upload alongside the dest parent.
    }

    // ── 4. Upload the bundle to the destination ─────────────────────────
    onStage?.('uploading')
    const uploaded = await deps.daemonCliPost<{ path: string }>('fs/upload-binary', {
      dir: uploadDir,
      filename: basename(bundle.bundle_path),
      base64,
    })
    if (!uploaded?.path) {
      throw new Error('Upload step returned no remote path.')
    }

    // ── 5. Unpack on the destination ────────────────────────────────────
    onStage?.('unpacking')
    const result = await deps.daemonCliPost<CloneUnpackResult>('clone/unpack', {
      bundle_path: uploaded.path,
      dest_parent: destParent,
    })
    if (!result?.dest_path) {
      throw new Error('Unpack step returned no dest_path.')
    }

    onStage?.('done')
    onDone?.(result)
    return result
  } catch (err: unknown) {
    const message = err instanceof Error ? err.message : String(err)
    onStage?.('error')
    onError?.(message)
    throw err
  }
}

/**
 * Build the default (real) dep bag for {@link cloneWorkspaceTo}. Bound
 * lazily inside the function bodies so importing this module in a headless
 * test env (vitest) never pulls Tauri/store side effects until called.
 *
 * `waitForHostConnected` resolves when the connect-host store reports the
 * destination host active AND connected, and rejects if the user cancels
 * sign-in (pendingSignIn cleared back to null without a connect) or a
 * timeout elapses.
 */
export function defaultCloneDeps(): CloneDeps {
  return {
    daemonCliPost: async (route, body) => {
      const { daemonCliPost } = await import('./daemon-cli')
      return daemonCliPost(route, body)
    },
    daemonCliGet: async (route, params) => {
      const { daemonCliGet } = await import('./daemon-cli')
      return daemonCliGet(route, params)
    },
    readLocalFileBase64: async (path) => {
      const { invoke } = await import('@tauri-apps/api/core')
      return invoke<string>('read_local_file_base64', { path })
    },
    pickHost: (host) => {
      void import('@/stores/connect-host').then(({ useConnectHostStore }) => {
        useConnectHostStore.getState().pickHost(host)
      })
    },
    waitForHostConnected: async (host, timeoutMs = 60000) => {
      const { useConnectHostStore, activeHostKey } = await import('@/stores/connect-host')
      const targetKey = activeHostKey(host)
      const store = useConnectHostStore
      // Already active + connected? Resolve immediately.
      const now = store.getState()
      if (
        activeHostKey(now.activeHost) === targetKey &&
        now.connectionStatus === 'connected'
      ) {
        return
      }
      await new Promise<void>((resolve, reject) => {
        let settled = false
        const finish = (fn: () => void): void => {
          if (settled) return
          settled = true
          clearTimeout(timer)
          unsub()
          fn()
        }
        const timer = setTimeout(() => {
          finish(() => reject(new CloneCancelledError(
            `Timed out connecting to ${host.label}.`,
          )))
        }, timeoutMs)
        const unsub = store.subscribe((state) => {
          const onTarget = activeHostKey(state.activeHost) === targetKey
          if (onTarget && state.connectionStatus === 'connected') {
            finish(resolve)
            return
          }
          // User cancelled the sign-in overlay: pendingSignIn was raised for
          // our host and then cleared back to null WITHOUT the host becoming
          // active. Treat as a cancel so we don't hang for the full timeout.
          if (
            !onTarget &&
            state.pendingSignIn === null &&
            activeHostKey(state.activeHost) === 'local'
          ) {
            finish(() => reject(new CloneCancelledError(
              `Sign-in to ${host.label} was cancelled.`,
            )))
          }
        })
      })
    },
    pickRemoteFolder: async () => {
      const { useRemoteFolderPickerStore } = await import('@/stores/remote-folder-picker')
      return useRemoteFolderPickerStore.getState().open()
    },
  }
}
