// Seti UI file-type glyph for the Files drawer.
// Font: jesseweed/seti-ui (MIT) via VS Code theme-seti. See NOTICE.md.
// Colors follow `useStyleStore.resolvedScheme` (light/dark).

import type { JSX } from 'react'
import setiWoffUrl from '@/assets/seti/seti.woff?url'
import { useStyleStore } from '@/stores/style'
import { resolveSetiIcon, type SetiScheme } from './resolve'

let fontReady: Promise<void> | null = null

function ensureSetiFont(): void {
  if (typeof document === 'undefined') return
  if (fontReady) return
  const id = 'k2-seti-font-face'
  if (!document.getElementById(id)) {
    const style = document.createElement('style')
    style.id = id
    style.textContent = `
@font-face {
  font-family: 'k2-seti';
  src: url('${setiWoffUrl}') format('woff');
  font-weight: normal;
  font-style: normal;
  font-display: block;
}
`.trim()
    document.head.appendChild(style)
  }
  if ('fonts' in document) {
    fontReady = document.fonts.load("16px 'k2-seti'").then(
      () => undefined,
      () => undefined,
    )
  } else {
    fontReady = Promise.resolve()
  }
}

export interface SetiFileIconProps {
  /** File basename (or path — basename is taken). Omit for default glyph. */
  name?: string
  className?: string
  title?: string
  /** Override scheme (tests). Default: live style store resolved scheme. */
  scheme?: SetiScheme
}

/**
 * Colored Seti file-type icon (16px). Folders use a separate folder mark in
 * FileTree — Seti theme is file-oriented.
 */
export function SetiFileIcon({
  name,
  className = 'w-4 h-4',
  title,
  scheme: schemeProp,
}: SetiFileIconProps): JSX.Element {
  ensureSetiFont()
  const storeScheme = useStyleStore((s) => s.resolvedScheme)
  const scheme: SetiScheme = schemeProp ?? (storeScheme === 'light' ? 'light' : 'dark')
  const def = resolveSetiIcon(name, scheme)
  const glyph = String.fromCodePoint(def.code)
  return (
    <span
      className={`inline-flex items-center justify-center flex-shrink-0 leading-none select-none ${className}`}
      style={{
        fontFamily: 'k2-seti, sans-serif',
        fontSize: '16px',
        color: def.color,
        width: '1rem',
        height: '1rem',
      }}
      title={title}
      aria-hidden
    >
      {glyph}
    </span>
  )
}
