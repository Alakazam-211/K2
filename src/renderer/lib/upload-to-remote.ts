// K2 Connect remote-files — upload substrate (client half).
//
// Tauri's native drag-drop gives the renderer a LOCAL file path on the
// user's machine but no bytes. When the active daemon is remote (K2
// Connect) it can't read that path — the file is on the client's disk.
// Two upload strategies, picked by file size (0.40.22 — the 100 MB
// ceiling fix):
//
//   - ≤ SINGLE_SHOT_MAX_BYTES: read whole (base64 via the
//     `read_local_file_base64` Tauri command), one POST to
//     `/cli/fs/upload-binary`. Works against any host version.
//   - larger: ordered 8 MB chunks via `read_local_file_range` →
//     `/cli/fs/upload-chunk` — the clone-to streaming substrate (GH #3),
//     now shared. Neither side ever holds the whole file; the daemon
//     appends to a temp `.part` and atomic-renames on the final chunk.
//     Bounded by the daemon's 10 GiB `MAX_TRANSFER_SIZE` + a free-disk
//     check on the first chunk (`total_bytes`).
//
// `uploadFileChunked` is the ONE chunk loop — `clone-to.ts` delegates to
// it too, so a transfer-layer fix lands in both features at once.

import { invoke } from '@tauri-apps/api/core'

import { daemonCliPost } from './daemon-cli'

/**
 * Files at or under this take the single-shot `fs/upload-binary` path —
 * universally supported (pre-0.40.6 hosts lack the chunk route) and well
 * under the daemon's 100 MB single-shot cap. 64 MB leaves headroom.
 */
export const SINGLE_SHOT_MAX_BYTES = 64 * 1024 * 1024
/**
 * Per-chunk decoded size for the streaming path. Kept small so each request
 * body stays tiny (the daemon buffers each one whole, and it sails through
 * the K2 Connect relay); well under the daemon's 16 MB per-chunk guard.
 */
export const UPLOAD_CHUNK_BYTES = 8 * 1024 * 1024

/** Raised when a transfer stops because `isCancelled` reported true. The
 *  daemon-side `.part` is abandoned; a later offset-0 restart truncates it. */
export class TransferCancelledError extends Error {
  readonly cancelled = true
  constructor(message = 'Transfer cancelled.') {
    super(message)
    this.name = 'TransferCancelledError'
  }
}

/** Byte-level observation + cooperative cancel, polled between chunks. */
export interface TransferHooks {
  onProgress?: (sentBytes: number, totalBytes: number) => void
  isCancelled?: () => boolean
}

/** The two side-effecting collaborators the chunk loop needs — injected so
 *  clone-to's mock-based sequencing tests drive the SAME loop. */
export interface ChunkedUploadDeps {
  daemonCliPost: <T = unknown>(route: string, body?: unknown) => Promise<T>
  readLocalFileRange: (path: string, offset: number, len: number) => Promise<string>
}

/** Last path segment of a local path, tolerating BOTH `/` and `\`
 *  separators (a Windows client path dropped onto a unix daemon). */
function basename(localPath: string): string {
  const seg = localPath.split(/[/\\]/).pop()
  return seg && seg.length > 0 ? seg : localPath
}

/**
 * Stream a large LOCAL file to the active daemon in ordered chunks via
 * `fs/upload-chunk`, returning the finalized remote path. Each chunk is
 * read from local disk and POSTed in turn, so neither the client nor the
 * daemon ever holds the whole (multi-GB) file in memory.
 *
 * `offset` advances by the REQUESTED chunk length: callers upload stable
 * files of known `size`, so a non-final read always returns a full chunk.
 * If reality ever diverges, the daemon's ordered-append check rejects the
 * next chunk LOUDLY rather than silently corrupting the file.
 *
 * The first chunk carries `total_bytes` so the daemon can refuse a doomed
 * transfer (over its ceiling / larger than free disk) before bytes move.
 */
export async function uploadFileChunked(
  deps: ChunkedUploadDeps,
  localPath: string,
  size: number,
  opts: {
    dir: string
    filename: string
    /** Prefixes the transfer id (diagnostics: whose `.part` is this). */
    idPrefix?: string
    chunkBytes?: number
  } & TransferHooks,
): Promise<string> {
  // Unique per-transfer id so a stale/abandoned `.part` can never be
  // appended to by a different transfer (collision-free among recent
  // uploads is enough, not cryptographic).
  const uploadId = `${opts.idPrefix ?? 'upload'}-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
  const chunkBytes = opts.chunkBytes ?? UPLOAD_CHUNK_BYTES
  let offset = 0
  let finalPath: string | undefined
  while (offset < size) {
    if (opts.isCancelled?.()) {
      throw new TransferCancelledError(`Upload of ${opts.filename} cancelled.`)
    }
    const len = Math.min(chunkBytes, size - offset)
    const isLast = offset + len >= size
    const base64 = await deps.readLocalFileRange(localPath, offset, len)
    const resp = await deps.daemonCliPost<{ path?: string; done?: boolean }>('fs/upload-chunk', {
      upload_id: uploadId,
      dir: opts.dir,
      filename: opts.filename,
      offset,
      base64,
      is_last: isLast,
      // Only meaningful on the offset-0 chunk (up-front viability check);
      // harmless-but-ignored on the rest, so send once.
      ...(offset === 0 ? { total_bytes: size } : {}),
    })
    offset += len
    opts.onProgress?.(offset, size)
    if (isLast) {
      if (!resp?.path) {
        throw new Error('Chunked upload returned no remote path.')
      }
      finalPath = resp.path
    }
  }
  if (finalPath === undefined) {
    // size === 0 → the loop never ran. Guard loudly rather than hand back
    // a bogus path.
    throw new Error('Chunked upload sent no data (empty file).')
  }
  return finalPath
}

/**
 * Move a local file's bytes onto the active daemon's disk, choosing the
 * single-shot or chunked strategy by size (see module doc).
 *
 * @param localPath absolute path to a file on the CLIENT machine (what
 *   Tauri drag-drop hands us).
 * @param destDir   destination directory ON THE DAEMON (e.g.
 *   `<workspace>/.k2so/downloads` for the terminal-drop case).
 * @returns the final absolute remote path the daemon wrote (collision
 *   handling may have appended ` (1)`, ` (2)`, …).
 */
export async function uploadToRemote(
  localPath: string,
  destDir: string,
  hooks: TransferHooks = {},
): Promise<string> {
  const filename = basename(localPath)
  const size = await invoke<number>('local_file_size', { path: localPath })

  if (size <= SINGLE_SHOT_MAX_BYTES) {
    if (hooks.isCancelled?.()) {
      throw new TransferCancelledError(`Upload of ${filename} cancelled.`)
    }
    const base64 = await invoke<string>('read_local_file_base64', {
      path: localPath,
    })
    const res = await daemonCliPost<{ path: string }>('fs/upload-binary', {
      dir: destDir,
      filename,
      base64,
    })
    hooks.onProgress?.(size, size)
    return res.path
  }

  return uploadFileChunked(
    {
      daemonCliPost,
      readLocalFileRange: (path, offset, len) =>
        invoke<string>('read_local_file_range', { path, offset, len }),
    },
    localPath,
    size,
    { dir: destDir, filename, idPrefix: 'drop', ...hooks },
  )
}
