/**
 * Renders modifier key symbols (⌘⇧⌥⌃↵) using the system font
 * so they display as crisp Apple-style glyphs instead of ugly Unicode.
 */

const MODIFIER_CHARS = new Set(['⌘', '⇧', '⌥', '⌃', '↵', '⏎', '⌫', '⎋', '⇥'])

interface KeyComboProps {
  combo: string
  className?: string
}

/** Render a key combo string with system-font modifier symbols */
export function KeyCombo({ combo, className = '' }: KeyComboProps): React.JSX.Element {
  // Split into individual characters and wrap modifiers in system font spans
  const parts: React.JSX.Element[] = []
  let i = 0

  for (const char of combo) {
    if (MODIFIER_CHARS.has(char)) {
      parts.push(<span key={i} className="key-symbol">{char}</span>)
    } else {
      parts.push(<span key={i}>{char}</span>)
    }
    i++
  }

  return <span className={className}>{parts}</span>
}

/** Simple inline modifier symbol with system font */
export function Mod({ children }: { children: string }): React.JSX.Element {
  return <span className="key-symbol">{children}</span>
}
