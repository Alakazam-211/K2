// A paste of files into a message box (a screenshot from Cmd+Ctrl+Shift+4,
// an image copied in a browser, a file copied in Finder).
//
// WKWebView (the macOS desktop app) hands a pasted image over as
// `clipboardData.files` (one File, named `image.png`); Chromium and the
// web client may fill only `items` (kind "file"). Both are read here.
//
// One rule for every box that takes files on paste (the agent Thread's
// box, a ticket's box): files on the clipboard are attached through the
// box's own attach path (the same as the file picker), and text pastes as
// text. When the clipboard carries both, the text still pastes unless it
// is only the files' names (a Finder copy) or the image's own address (a
// browser's Copy Image): those would be noise beside the chip.

/** Browser files on a clipboard (a pasted screenshot). */
export function filesOfClipboard(data: Pick<DataTransfer, 'files' | 'items'> | null | undefined): File[] {
  if (!data) return []
  const out: File[] = []
  if (data.files && data.files.length > 0) {
    for (let i = 0; i < data.files.length; i++) out.push(data.files[i])
    return out
  }
  const items = data.items
  if (!items) return out
  for (let i = 0; i < items.length; i++) {
    const it = items[i]
    if (it.kind !== 'file') continue
    const f = it.getAsFile()
    if (f) out.push(f)
  }
  return out
}

const IMAGE_EXT: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/jpg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/tiff': 'tiff',
  'image/heic': 'heic',
  'image/bmp': 'bmp',
  'image/svg+xml': 'svg',
}

/** A clipboard image's own name says nothing (`image.png`, or none). */
function isGenericImageName(f: File): boolean {
  const name = f.name ?? ''
  if (name === '') return f.type.startsWith('image/')
  return /^image(\.[a-z0-9]+)?$/i.test(name)
}

function pad(n: number): string {
  return String(n).padStart(2, '0')
}

/** `pasted-image-20261123-142501.png` (`-2`, `-3` for more in one paste):
 *  every paste uploads under its own name, so a second screenshot never
 *  lands on the first one's path. */
export function pastedImageName(f: File, now: Date, n: number): string {
  const fromName = /\.([a-z0-9]+)$/i.exec(f.name ?? '')?.[1]?.toLowerCase()
  const ext = IMAGE_EXT[f.type] ?? fromName ?? 'png'
  const stamp =
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}` +
    `-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`
  return `pasted-image-${stamp}${n > 1 ? `-${n}` : ''}.${ext}`
}

function pathOf(f: File): string | undefined {
  const p = (f as unknown as { path?: unknown }).path
  return typeof p === 'string' && p !== '' ? p : undefined
}

/** Is the pasted text only what the files already say (their names or
 *  paths, one per line), or one URL (the copied image's address)? */
function textIsNoise(text: string, files: File[]): boolean {
  const lines = text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l !== '')
  if (lines.length === 0) return true
  if (lines.length === 1 && /^(https?|file|data|blob):\S*$/i.test(lines[0])) return true
  const said = new Set<string>()
  for (const f of files) {
    if (f.name) said.add(f.name)
    const p = pathOf(f)
    if (p) said.add(p)
  }
  return lines.every((l) => said.has(l) || said.has(l.split('/').pop() ?? l))
}

export interface ComposePaste {
  /** Files with a path on this computer (an Electron-style File). */
  paths: string[]
  /** Browser files to upload, clipboard images renamed to be unique. */
  files: File[]
  /** The clipboard's text pastes too (leave the event's default alone). */
  keepText: boolean
}

/** What a paste carries for a box that takes files, or null when there
 *  are no files on the clipboard (a text paste, left to the field). */
export function composePaste(
  data: Pick<DataTransfer, 'files' | 'items' | 'getData'> | null | undefined,
  now: Date = new Date(),
): ComposePaste | null {
  const found = filesOfClipboard(data)
  if (found.length === 0) return null
  const paths: string[] = []
  const files: File[] = []
  let images = 0
  for (const f of found) {
    const p = pathOf(f)
    if (p) {
      paths.push(p)
      continue
    }
    if (isGenericImageName(f)) {
      images += 1
      files.push(new File([f], pastedImageName(f, now, images), { type: f.type, lastModified: f.lastModified }))
    } else {
      files.push(f)
    }
  }
  const text = data?.getData?.('text/plain') ?? ''
  return { paths, files, keepText: !textIsNoise(text, found) }
}
