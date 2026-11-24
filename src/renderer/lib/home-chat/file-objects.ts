// A Home chat message's files as the Thread draws them
// (contract-home-chat-attachments-v1). The Thread's own pieces show them,
// unchanged: chips, list card, image cards, the image viewer and Download
// (`ThreadFileObjects`, under `ThreadChatBubble`), read through a Home chat
// door (`homeChatAttachmentSource`): every byte from this server's daemon
// as the person (`GET /cli/home-chat/file`), never from Sew.

import type { ServerScope } from '@/kessel/server-scope'
import type { ThreadFileObject } from '@/components/SessionView/overlayThread'
import type { ThreadAttachmentSource } from '@/components/Thread/ThreadImageObjects'
import type { AttachmentReader } from '@/lib/thread-image-thumbs'
import { bytesToBase64 } from '@/lib/upload-to-remote'
import { HOME_CHAT_ATTACHMENTS_FEATURE, fetchHomeChatFile, type HomeChatAttachment, type HomeChatFileVariant } from './api'

/** Originals a browser may draw as themselves. */
const SAFE_IMAGES = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp'])

function baseType(t: string): string {
  return t.toLowerCase().split(';')[0].trim()
}

/** The message's files as the Thread's file objects (index = `n`). */
export function homeChatFileObjects(atts: readonly HomeChatAttachment[] | undefined): ThreadFileObject[] {
  return (atts ?? []).map((a) => ({
    kind: 'file',
    state: 'ok',
    path: a.filename,
    name: a.filename,
    size: a.size,
    ...(a.contentType ? { mime: baseType(a.contentType) } : {}),
    // A person may save what they may read (the daemon checks as them).
    canDownload: true,
  }))
}

/** Which variant draws a picture (`thumb`: the card; `preview`: the viewer),
 *  or null when there is none to draw. */
export function pictureVariant(a: HomeChatAttachment, mode: 'thumb' | 'preview'): HomeChatFileVariant | null {
  const safe = SAFE_IMAGES.has(baseType(a.contentType))
  if (mode === 'thumb') return a.thumb ? 'thumb' : safe ? 'original' : null
  if (a.preview) return 'preview'
  if (safe) return 'original'
  return a.thumb ? 'thumb' : null
}

/** The Thread door's answers, from Home chat's file route: a picture
 *  (`mode=thumb|preview`) or a range of the file (`mode=download`). */
export function homeChatAttachmentReader(
  scope: ServerScope,
  filesOf: (messageId: string) => readonly HomeChatAttachment[] | undefined,
  fetchFile: typeof fetchHomeChatFile = fetchHomeChatFile,
): AttachmentReader {
  return async (docId, n, params) => {
    const a = filesOf(docId)?.[n]
    if (!a) throw new Error('this file is no longer on the message')
    if (params.mode === 'download') {
      const offset = Number(params.offset ?? 0)
      const len = Math.max(1, Math.min(Number(params.len ?? a.size), a.size - offset))
      const blob = await fetchFile(scope, a.id, { download: true, range: { offset, len } })
      const bytes = new Uint8Array(await blob.arrayBuffer())
      return { base64: bytesToBase64(bytes), len: bytes.length, size: a.size, eof: offset + bytes.length >= a.size }
    }
    const mode = params.mode === 'preview' ? 'preview' : 'thumb'
    const variant = pictureVariant(a, mode)
    // No picture: the Thread's card falls back to its chip (415, settled).
    if (!variant) return { kind: 'none' }
    const blob = await fetchFile(scope, a.id, { variant })
    const bytes = new Uint8Array(await blob.arrayBuffer())
    return {
      kind: 'image',
      base64: bytesToBase64(bytes),
      mime: baseType(blob.type || a.contentType),
      width: a.width,
      height: a.height,
    }
  }
}

/** The door for one server's Home chat files. Memoize it per (scope). */
export function homeChatAttachmentSource(
  scope: ServerScope,
  filesOf: (messageId: string) => readonly HomeChatAttachment[] | undefined,
): ThreadAttachmentSource {
  return {
    scope,
    read: homeChatAttachmentReader(scope, filesOf),
    key: `home-chat:${scope.hostKey}`,
    imagesFeature: HOME_CHAT_ATTACHMENTS_FEATURE,
  }
}
