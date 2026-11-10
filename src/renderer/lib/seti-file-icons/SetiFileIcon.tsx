// Seti UI file-type glyph for the Files drawer.
// Font: jesseweed/seti-ui (MIT) via VS Code theme-seti. See NOTICE.md.
// Colors follow `useStyleStore.resolvedScheme` (light/dark).

import type { JSX } from 'react'
import setiWoffUrl from '@/assets/seti/seti.woff?url'
import { useStyleStore } from '@/stores/style'
import { FEATURE_ICONS } from '@/components/icons/feature-icons'
import { resolveSetiIcon, type SetiScheme } from './resolve'
import { createSetiFontLoader, type SetiFontEnv, type SetiFontLoader } from './seti-font'

let loader: SetiFontLoader | null = null

/** Inject the Seti @font-face once; retries with a fresh rule if a load fails. */
function ensureSetiFont(): void {
  if (typeof document === 'undefined') return
  if (!loader) {
    loader = createSetiFontLoader(setiWoffUrl, {
      document: document as SetiFontEnv['document'],
      setTimeout: (fn, ms) => window.setTimeout(fn, ms),
    })
  }
  loader.ensure()
}

/** A Board file (`.k5board`, any case), by basename or path. */
function isBoardFileName(name: string | undefined): boolean {
  return !!name && name.toLowerCase().endsWith('.k5board')
}

export interface SetiFileIconProps {
  /** File basename (or path — basename is taken). Omit for default glyph. */
  name?: string
  className?: string
  title?: string
  /** Override scheme (tests). Default: live style store resolved scheme. */
  scheme?: SetiScheme
  /** Box in pixels. Default 16, the same size as the Files drawer. */
  size?: number
}

/**
 * Colored Seti file-type icon. Folders use a separate folder mark in
 * FileTree — Seti theme is file-oriented.
 */
export function SetiFileIcon({
  name,
  className = 'w-4 h-4',
  title,
  scheme: schemeProp,
  size = 16,
}: SetiFileIconProps): JSX.Element {
  ensureSetiFont()
  const storeScheme = useStyleStore((s) => s.resolvedScheme)
  const scheme: SetiScheme = schemeProp ?? (storeScheme === 'light' ? 'light' : 'dark')
  const def = resolveSetiIcon(name, scheme)
  const px = `${size}px`
  if (isBoardFileName(name)) {
    // A Board file draws the Board's own icon (one source: feature-icons), in
    // Seti's picture colour, so it never looks like Design or a picture.
    return (
      <span
        data-board-file-icon=""
        className={`inline-flex items-center justify-center flex-shrink-0 leading-none select-none ${className}`}
        style={{ color: def.color, width: px, height: px }}
        title={title}
        aria-hidden
      >
        <FEATURE_ICONS.board size={Math.round(size * 0.875)} />
      </span>
    )
  }
  const glyph = String.fromCodePoint(def.code)
  return (
    <span
      className={`inline-flex items-center justify-center flex-shrink-0 leading-none select-none ${className}`}
      style={{
        fontFamily: 'k2-seti, sans-serif',
        fontSize: px,
        color: def.color,
        width: px,
        height: px,
      }}
      title={title}
      aria-hidden
    >
      {glyph}
    </span>
  )
}
