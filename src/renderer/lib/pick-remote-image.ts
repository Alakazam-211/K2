// Host-aware chokepoint for "pick an IMAGE file for an icon".
//
// When connected to a REMOTE daemon, a native <input type=file> opens the
// LOCAL OS picker — the chosen file lives on this machine, but the icon
// flow needs an image the user can see on the HOST. So icon-upload entry
// points branch on the active host:
//
//   - activeHost === 'local' → the existing hidden <input type=file>
//   - remote                 → RemoteFolderPicker in FILE mode over the
//                              host's fs, then fs/read-binary for bytes
//
// The chosen bytes come back as a data URL, which feeds the SAME
// crop→set-icon pipeline the local path uses (the crop dialog downscales
// to a small PNG, so backends never see oversized payloads).

import { daemonCliGet } from '@/lib/daemon-cli'
import { useConnectHostStore } from '@/stores/connect-host'
import { useRemoteFolderPickerStore } from '@/stores/remote-folder-picker'
import { useToastStore } from '@/stores/toast'

/** Extension → MIME for the image types the picker accepts. */
const IMAGE_MIME_BY_EXT: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  svg: 'image/svg+xml',
  bmp: 'image/bmp',
  heic: 'image/heic',
}

function extOf(name: string): string {
  const idx = name.lastIndexOf('.')
  if (idx < 0 || idx === name.length - 1) return ''
  return name.slice(idx + 1).toLowerCase()
}

/** True when `name` looks like an image we can turn into an icon
 *  (png/jpg/jpeg/gif/webp/svg/bmp/heic, case-insensitive). */
export function isImageFileName(name: string): boolean {
  return extOf(name) in IMAGE_MIME_BY_EXT
}

/** Infer the data-URL MIME from a filename/path extension.
 *  Falls back to image/png for unknown-but-accepted-elsewhere inputs. */
export function imageMimeFromPath(path: string): string {
  const name = path.split('/').pop()?.split('\\').pop() ?? path
  return IMAGE_MIME_BY_EXT[extOf(name)] ?? 'image/png'
}

/**
 * Open the remote picker in file mode (image filter), read the chosen
 * file's bytes over the host-aware daemon route, and return them as a
 * `data:<mime>;base64,…` URL. Returns null on cancel, and null (after a
 * toast) when the read fails — e.g. the daemon's ~2MB read-binary cap.
 */
export async function pickRemoteImageDataUrl(): Promise<string | null> {
  const path = await useRemoteFolderPickerStore.getState().open({
    mode: 'file',
    accept: isImageFileName,
    title: 'Choose Image on Host',
  })
  if (!path) return null
  try {
    const r = await daemonCliGet<{ base64: string }>('fs/read-binary', { path })
    return `data:${imageMimeFromPath(path)};base64,${r.base64}`
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err)
    useToastStore
      .getState()
      .addToast(`Couldn't read that image from the host: ${msg}`, 'error')
    return null
  }
}

/**
 * The shared icon-upload click handler. Local host → click the hidden
 * native <input type=file> (unchanged flow). Remote host → remote file
 * picker → data URL → hand to the crop dialog via `setCropImage`.
 */
export async function pickIconImage(deps: {
  clickNativeInput: () => void
  setCropImage: (dataUrl: string) => void
}): Promise<void> {
  if (useConnectHostStore.getState().activeHost === 'local') {
    deps.clickNativeInput()
    return
  }
  const dataUrl = await pickRemoteImageDataUrl()
  if (dataUrl) deps.setCropImage(dataUrl)
}
