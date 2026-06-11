/**
 * Heart-shape outline with an EKG trace line passing horizontally
 * through the middle. Used as the sidebar Heartbeats panel header
 * icon — the trace dropping through the heart visually echoes the
 * "scheduled wakeups" concept without leaning on emoji.
 *
 * Sized by the parent's CSS (className). Inherits stroke colour from
 * `currentColor` so the icon picks up the panel's text colour.
 */
export function IconHeartEKG({ className }: { className?: string }): React.JSX.Element {
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
      {/* Heart outline */}
      <path d="M8 13.5 C5.5 11 1.5 9 1.5 5.5 a3 3 0 0 1 6 -1 a3 3 0 0 1 6 1 C14.5 9 10.5 11 8 13.5 z" />
      {/* EKG trace passing through the middle of the heart */}
      <path d="M0.5 8 H4 L5 6 L6.5 10 L8 4 L9.5 10 L11 6 L12 8 H15.5" />
    </svg>
  )
}
