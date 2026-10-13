// An agent's look (0.45.5 agent identity: `projects.color`, `projects.icon_url`)
// as a Garden widget may see it. The same rules the daemon applies to the App
// surface (`GET /cli/skin/agents`, `k2_core::workspace::agent_look`):
//   - color: `#rgb` / `#rrggbb` → lowercase `#rrggbb`, anything else null;
//   - image: a PNG / JPEG / WebP / GIF `data:` URL whose magic bytes match its
//     type and that decodes to at most 256 KiB, else null. Never SVG (it can
//     carry script), never a file or http URL (a local address can't load in
//     a sealed frame and would leak where it lives).

/** `MAX_WIRE_IMAGE_BYTES` in `agent_look.rs`. */
export const AGENT_LOOK_MAX_IMAGE_BYTES = 256 * 1024

/** The agent's color as `#rrggbb`, or null. */
export function agentLookColor(raw: string | null | undefined): string | null {
  if (typeof raw !== 'string') return null
  const t = raw.trim()
  const hex = t.startsWith('#') ? t.slice(1) : t
  if (!/^[0-9a-fA-F]+$/.test(hex)) return null
  if (hex.length === 6) return `#${hex.toLowerCase()}`
  if (hex.length === 3) {
    const h = hex.toLowerCase()
    return `#${h[0]}${h[0]}${h[1]}${h[1]}${h[2]}${h[2]}`
  }
  return null
}

type Raster = 'png' | 'jpeg' | 'webp' | 'gif'

const MIME_RASTER: Record<string, Raster> = {
  'image/png': 'png',
  'image/jpeg': 'jpeg',
  'image/jpg': 'jpeg',
  'image/webp': 'webp',
  'image/gif': 'gif',
}

function sniff(b: Uint8Array): Raster | null {
  const starts = (sig: readonly number[]): boolean => sig.every((v, i) => b[i] === v)
  if (starts([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return 'png'
  if (starts([0xff, 0xd8, 0xff])) return 'jpeg'
  if (b.length >= 12 && starts([0x52, 0x49, 0x46, 0x46]) && b[8] === 0x57 && b[9] === 0x45 && b[10] === 0x42 && b[11] === 0x50)
    return 'webp'
  if (starts([0x47, 0x49, 0x46, 0x38]) && (b[4] === 0x37 || b[4] === 0x39) && b[5] === 0x61) return 'gif'
  return null
}

const B64 = /^[A-Za-z0-9+/]*={0,2}$/

function decodeHead(payload: string, chars: number): Uint8Array | null {
  try {
    const bin = atob(payload.slice(0, chars))
    const out = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
    return out
  } catch {
    return null
  }
}

/** Rows are rebuilt on every status change; the check is cached per string. */
const imageCache = new Map<string, string | null>()
const IMAGE_CACHE_MAX = 64

function checkImage(raw: string): string | null {
  if (!raw.startsWith('data:')) return null
  const comma = raw.indexOf(',')
  if (comma < 0) return null
  const head = raw.slice(5, comma)
  if (!head.endsWith(';base64')) return null
  const want = MIME_RASTER[head.slice(0, -';base64'.length).trim().toLowerCase()]
  if (!want) return null
  const payload = raw.slice(comma + 1)
  if (payload.length === 0 || payload.length % 4 !== 0) return null
  if (payload.length > (AGENT_LOOK_MAX_IMAGE_BYTES / 3) * 4 + 8) return null
  if (!B64.test(payload)) return null
  const pad = payload.endsWith('==') ? 2 : payload.endsWith('=') ? 1 : 0
  if ((payload.length / 4) * 3 - pad > AGENT_LOOK_MAX_IMAGE_BYTES) return null
  const bytes = decodeHead(payload, 16)
  if (!bytes || sniff(bytes) !== want) return null
  return raw
}

/** The agent's picture as a widget may get it, or null. */
export function agentLookImage(raw: string | null | undefined): string | null {
  if (typeof raw !== 'string') return null
  const t = raw.trim()
  if (!t) return null
  const hit = imageCache.get(t)
  if (hit !== undefined) return hit
  const out = checkImage(t)
  if (imageCache.size >= IMAGE_CACHE_MAX) imageCache.clear()
  imageCache.set(t, out)
  return out
}

/** Initials text that reads on `#rrggbb` (WCAG relative luminance). */
export function agentLookTextOn(color: string): string {
  const c = agentLookColor(color)
  if (!c) return '#ffffff'
  const ch = (i: number): number => {
    const v = parseInt(c.slice(i, i + 2), 16) / 255
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4
  }
  const l = 0.2126 * ch(1) + 0.7152 * ch(3) + 0.0722 * ch(5)
  // Contrast with white vs black; pick the larger.
  return 1.05 / (l + 0.05) >= (l + 0.05) / 0.05 ? '#ffffff' : '#111111'
}
