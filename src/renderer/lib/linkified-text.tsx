// Render plain text with clickable http(s) URLs. Used by Tickets + Project
// chat so threads stay selectable/copyable while links open externally.

import React, { type CSSProperties } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'

const URL_RE = /(https?:\/\/[^\s<>"'`]+)/gi

function trimTrailingPunct(url: string): { href: string; trail: string } {
  // Keep balanced path punctuation; strip common sentence closers.
  let href = url
  let trail = ''
  while (/[.,);:!?]$/.test(href) && !href.endsWith(')/')) {
    trail = href.slice(-1) + trail
    href = href.slice(0, -1)
  }
  return { href, trail }
}

/** Inline select styles — beats body.userSelect=none left by resize drags. */
export const SELECTABLE_TEXT_STYLE: CSSProperties = {
  userSelect: 'text',
  WebkitUserSelect: 'text',
  cursor: 'text',
}

export function LinkifiedText({
  text,
  className,
}: {
  text: string
  className?: string
}): React.JSX.Element {
  const parts = text.split(URL_RE)
  return (
    <span
      className={className ?? 'selectable-copy whitespace-pre-wrap break-words'}
      style={SELECTABLE_TEXT_STYLE}
    >
      {parts.map((part, i) => {
        if (i % 2 === 1 || /^https?:\/\//i.test(part)) {
          const { href, trail } = trimTrailingPunct(part)
          return (
            <React.Fragment key={i}>
              <a
                href={href}
                className="text-[var(--color-accent)] underline underline-offset-2 hover:opacity-90 cursor-pointer break-all selectable-copy"
                style={SELECTABLE_TEXT_STYLE}
                onClick={(e) => {
                  e.preventDefault()
                  e.stopPropagation()
                  void openUrl(href).catch(() => {
                    // Fallback for non-Tauri / web: try window.open.
                    window.open(href, '_blank', 'noopener,noreferrer')
                  })
                }}
              >
                {href}
              </a>
              {trail}
            </React.Fragment>
          )
        }
        return <React.Fragment key={i}>{part}</React.Fragment>
      })}
    </span>
  )
}
