// Files a person attaches in a Thread's message box (S4, daemon feature
// `thread-attachments-v1`; eval-thread-file-objects-v1 §6 S4).
//
// On a server with the feature, a picked or dropped file rides the Thread
// post as an attachment (`POST /cli/thread/post {…, attachments:[{path}]}`)
// instead of path text in the message; the daemon checks each one (inside
// the Thread's workspace, a regular file, not a secret) and still hands the
// agent `Attached: <abs path>` lines in its PTY inject. Which path:
//   - a file on the Thread's own server that is already inside the
//     workspace: that path, as it is;
//   - anything else (a local file outside the workspace, any file when the
//     Thread is on another server, a browser `File`): uploaded into the
//     workspace's `.k2/downloads/` first (the same upload the compose bar
//     does today, `compose-attach.ts`), and that path is attached.
// Without the feature the compose bar keeps today's path text.

import type { ServerScope } from '@/kessel/server-scope'
import { httpCodeOf } from '@/lib/daemon-http-error'

/** One file waiting in the message box: a path on the Thread's server. */
export interface ThreadComposeFile {
  path: string
  name: string
}

/** The daemon refuses more than this many on one post. */
export const THREAD_POST_MAX_FILES = 10

/** Said when more than `THREAD_POST_MAX_FILES` would ride one message. */
export const THREAD_POST_TOO_MANY_FILES = `At most ${THREAD_POST_MAX_FILES} files per message.`

/** Does the Thread's server take attachments on a compose post? */
export function threadComposeAttachOn(scope: Pick<ServerScope, 'serverSupports'>): boolean {
  return scope.serverSupports('thread-attachments-v1')
}

function trimSlash(p: string): string {
  return p.length > 1 ? p.replace(/[/\\]+$/, '') : p
}

/** Is `path` the workspace folder's own file (anywhere under it)? */
export function isInsideWorkspace(path: string, workspacePath: string | undefined): boolean {
  if (!workspacePath) return false
  const root = trimSlash(workspacePath)
  const p = path.replace(/\\/g, '/')
  const r = root.replace(/\\/g, '/')
  return p.startsWith(`${r}/`) && p.length > r.length + 1
}

function fileName(path: string): string {
  return path.split(/[/\\]/).filter(Boolean).pop() || path
}

export function threadComposeFile(path: string): ThreadComposeFile {
  return { path, name: fileName(path) }
}

/** Add `more` to `cur`: no path twice, at most `THREAD_POST_MAX_FILES`.
 *  `dropped` counts what didn't fit. */
export function mergeThreadComposeFiles(
  cur: readonly ThreadComposeFile[],
  more: readonly ThreadComposeFile[],
): { files: ThreadComposeFile[]; dropped: number } {
  const out = [...cur]
  let dropped = 0
  for (const f of more) {
    if (out.some((o) => o.path === f.path)) continue
    if (out.length >= THREAD_POST_MAX_FILES) {
      dropped++
      continue
    }
    out.push(f)
  }
  return { files: out, dropped }
}

/** The capture an upload hands back: the server paths it wrote. */
function captureUpload(): { build: (remote: string[]) => string; paths: () => string[] | null } {
  let got: string[] | null = null
  return {
    build: (remote) => {
      got = [...remote]
      return remote.join('\n') || '\n'
    },
    paths: () => got,
  }
}

/** An upload of the same files that was already running answered (its
 *  own payload, not ours): we can't tell which paths it wrote. */
const UPLOAD_JOINED = 'Those files are already uploading. Attach them again once the upload finishes.'

export interface ThreadComposeAttachInput {
  /** Paths on THIS computer (the native picker, a Tauri drop). */
  paths?: string[]
  /** Browser files with no path. */
  files?: File[]
  /** The Thread's workspace on its server. */
  workspacePath?: string
}

/**
 * Turn picked or dropped files into paths on the Thread's server to attach
 * (see the header). Null when nothing was attached: the picker or upload was
 * cancelled, or the upload failed (the upload path already toasted why).
 */
export async function resolveThreadComposeFiles(
  scope: ServerScope,
  input: ThreadComposeAttachInput,
): Promise<ThreadComposeFile[] | null> {
  // Loaded on use: the upload path pulls in the transfer stores, and this
  // module is also read by the light Thread post helper (`overlayThread`).
  const { executeBrowserFileDrop, executeRemoteDrop } = await import('@/lib/handle-remote-drop')
  const ctx = { workspacePath: input.workspacePath || undefined }
  if (input.paths && input.paths.length > 0) {
    const paths = input.paths
    // This computer is the Thread's server: a file already in the
    // workspace is attached where it is; only the others are copied in.
    const keep = scope.isRemote ? [] : paths.filter((p) => isInsideWorkspace(p, input.workspacePath))
    const upload = paths.filter((p) => !keep.includes(p))
    let uploaded: string[] = []
    if (upload.length > 0) {
      const cap = captureUpload()
      const payload = await executeRemoteDrop(scope, upload, { kind: 'terminal' }, ctx, cap.build)
      if (payload === null) return null
      const got = cap.paths()
      if (!got) throw new Error(UPLOAD_JOINED)
      uploaded = got
    }
    // Keep the order the person picked them in.
    const byLocal = new Map<string, string>()
    upload.forEach((p, i) => {
      if (uploaded[i]) byLocal.set(p, uploaded[i])
    })
    const out: ThreadComposeFile[] = []
    for (const p of paths) {
      const server = keep.includes(p) ? p : byLocal.get(p)
      if (server) out.push(threadComposeFile(server))
    }
    return out.length > 0 ? out : null
  }
  if (input.files && input.files.length > 0) {
    const cap = captureUpload()
    const payload = await executeBrowserFileDrop(scope, input.files, { kind: 'terminal' }, ctx, cap.build)
    if (payload === null) return null
    const got = cap.paths()
    if (!got || got.length === 0) return null
    return got.map(threadComposeFile)
  }
  return null
}

/** What the person reads when the daemon refused the files on a post and
 *  sent no hint of its own. */
const ATTACHMENT_REFUSALS: Record<string, string> = {
  attachment_outside_workspace:
    "A file isn't inside this agent's workspace. Copy it into the workspace (e.g. out/) and attach it again.",
  attachment_not_a_file: "A file can't be attached: it is missing, or it is a folder.",
  attachment_secret: 'A file looks like a secret. Secrets travel through the vault (k2 vault), not the Thread.',
}

/** The refusal of a post with files, in words: the daemon's hint when it
 *  sent one, else ours for its code. Null when it isn't an attachment
 *  refusal. */
export function threadAttachmentRefusalText(err: unknown, fallback: string): string | null {
  const code = httpCodeOf(err)
  if (!code || !code.startsWith('attachment_')) return null
  const looksRaw = !fallback || fallback.trim().startsWith('{')
  return looksRaw ? ATTACHMENT_REFUSALS[code] ?? 'The server refused the attached files.' : fallback
}

// ── Draft persistence (thin client, beside the text draft) ───────────────

export function readThreadComposeFiles(key: string): ThreadComposeFile[] {
  try {
    const raw = localStorage.getItem(key)
    if (!raw) return []
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed
      .filter((f): f is ThreadComposeFile => !!f && typeof f === 'object' && typeof (f as ThreadComposeFile).path === 'string')
      .map((f) => threadComposeFile(f.path))
      .slice(0, THREAD_POST_MAX_FILES)
  } catch {
    return []
  }
}

export function writeThreadComposeFiles(key: string, files: readonly ThreadComposeFile[]): void {
  try {
    if (files.length > 0) localStorage.setItem(key, JSON.stringify(files.map((f) => ({ path: f.path }))))
    else localStorage.removeItem(key)
  } catch {
    /* storage disabled/full: the files just won't survive a restart */
  }
}
