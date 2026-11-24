// A file's upload into a Home DM or channel (contract-home-chat-attachments-v1):
// create on the conversation, then its bytes in ranges through the server's
// daemon (never to Sew directly), resuming from where the server stands
// after a cut. The message then names the upload ids (`send` attachments).

import type { ServerScope } from '@/kessel/server-scope'
import {
  createHomeChatUpload,
  fetchHomeChatUploadStatus,
  sendHomeChatUploadBytes,
  type HomeChatUpload,
} from './api'

/** Bytes to upload: a browser File, or a file on this computer. */
export interface HomeChatUploadSource {
  name: string
  size: number
  /** The declared type (the server sniffs its own). */
  type: string
  read: (offset: number, len: number) => Promise<Uint8Array>
}

/** A browser File (a pick, a drop, a paste). */
export function fileUploadSource(f: File): HomeChatUploadSource {
  return {
    name: f.name || 'pasted file',
    size: f.size,
    type: f.type,
    read: async (offset, len) => new Uint8Array(await f.slice(offset, offset + len).arrayBuffer()),
  }
}

function decodeBase64(b64: string): Uint8Array {
  const bin = atob(b64.replace(/\s+/g, ''))
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

/** The extension's type for a path (advice only: the server sniffs). */
export function typeForName(name: string): string {
  const ext = name.toLowerCase().split('.').pop() ?? ''
  const t: Record<string, string> = {
    png: 'image/png',
    jpg: 'image/jpeg',
    jpeg: 'image/jpeg',
    gif: 'image/gif',
    webp: 'image/webp',
    heic: 'image/heic',
    pdf: 'application/pdf',
    mp4: 'video/mp4',
    mov: 'video/quicktime',
    mp3: 'audio/mpeg',
    txt: 'text/plain',
    zip: 'application/zip',
  }
  return t[ext] ?? ''
}

/** A file on this computer (a native pick or a Finder drop), read by range
 *  through the desktop shell. */
export async function localPathUploadSource(path: string): Promise<HomeChatUploadSource> {
  const { invoke } = await import('@tauri-apps/api/core')
  const size = await invoke<number>('local_file_size', { path })
  const name = path.split('/').pop()?.split('\\').pop() || path
  return {
    name,
    size,
    type: typeForName(name),
    read: async (offset, len) => decodeBase64(await invoke<string>('read_local_file_range', { path, offset, len })),
  }
}

/** Images the chip draws as a picture (and Sew thumbnails). */
export function isPreviewableImage(name: string, type: string): boolean {
  const t = type.toLowerCase()
  if (t) return t === 'image/png' || t === 'image/jpeg' || t === 'image/gif' || t === 'image/webp'
  return /\.(png|jpe?g|gif|webp)$/i.test(name)
}

export interface HomeChatUploadApi {
  create: typeof createHomeChatUpload
  send: typeof sendHomeChatUploadBytes
  status: typeof fetchHomeChatUploadStatus
  sleep: (ms: number) => Promise<void>
}

const realApi: HomeChatUploadApi = {
  create: createHomeChatUpload,
  send: sendHomeChatUploadBytes,
  status: fetchHomeChatUploadStatus,
  sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
}

/** Most bytes sent in one request (whole chunks, under the server's cap). */
export const UPLOAD_REQUEST_BYTES = 4 << 20

/** Tries after a failed range before the upload fails. */
export const UPLOAD_RETRIES = 3

export class UploadAbortedError extends Error {
  constructor() {
    super('upload stopped')
    this.name = 'UploadAbortedError'
  }
}

/** Upload `src` into `conv`; resolves to the upload id. `onCreated` gets the
 *  id as soon as the server has one (so a removed chip can abort it). */
export async function uploadHomeChatFile(
  scope: ServerScope,
  conv: string,
  src: HomeChatUploadSource,
  hooks: { onProgress?: (ratio: number) => void; onCreated?: (id: string) => void; signal?: AbortSignal } = {},
  api: HomeChatUploadApi = realApi,
): Promise<string> {
  const stopped = (): void => {
    if (hooks.signal?.aborted) throw new UploadAbortedError()
  }
  stopped()
  const up: HomeChatUpload = await api.create(scope, { conv, filename: src.name, size: src.size, contentType: src.type || undefined })
  hooks.onCreated?.(up.id)
  const chunk = Math.max(1, up.chunkBytes)
  const step = Math.max(chunk, Math.floor(Math.min(UPLOAD_REQUEST_BYTES, up.maxRequestBytes) / chunk) * chunk)
  let committed = up.committed
  let complete = up.complete
  let tries = 0
  hooks.onProgress?.(src.size > 0 ? committed / src.size : 0)
  while (!complete && committed < src.size) {
    stopped()
    const len = Math.min(step, src.size - committed)
    try {
      const bytes = await src.read(committed, len)
      stopped()
      const r = await api.send(scope, up.id, committed, bytes)
      committed = r.committed
      complete = r.complete
      tries = 0
    } catch (err) {
      if (err instanceof UploadAbortedError) throw err
      tries += 1
      if (tries > UPLOAD_RETRIES) throw err
      await api.sleep(500 * tries)
      stopped()
      // A cut: continue from what the server has.
      const st = await api.status(scope, up.id)
      committed = st.committed
      complete = st.complete
    }
    hooks.onProgress?.(src.size > 0 ? committed / src.size : 1)
  }
  return up.id
}
