/**
 * Closed-padlock outline. Used to mark a K2 server connection as secure
 * (TLS / `https`) — the `secure` flag on a ConnectHost. A host reached
 * over plain `http` shows no lock, so this icon carries real, variable
 * state (secured vs not), not decoration.
 *
 * Matches the app's icon convention: an inline SVG component sized by the
 * parent's CSS (className) and stroked from `currentColor` so it inherits
 * the surrounding text colour.
 */
export function IconLock({ className }: { className?: string }): React.JSX.Element {
  return (
    <svg
      className={className}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {/* Shackle */}
      <path d="M5 7 V5 a3 3 0 0 1 6 0 V7" />
      {/* Body */}
      <rect x="3.5" y="7" width="9" height="6.5" rx="1" />
    </svg>
  )
}
