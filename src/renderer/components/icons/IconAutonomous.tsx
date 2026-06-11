/**
 * Autonomous-activity indicator — a small EKG/pulse glyph used on an
 * Active-bar item when the workspace surfaced itself by doing REAL WORK
 * during a heartbeat wake (self-driving), as opposed to a user-driven
 * session (which shows the braille spinner).
 *
 * Session-lifecycle P3: a heartbeat work-fire bumps the workspace's
 * `last_interaction_at` (so it enters the Active window) — this badge
 * is what lets the user tell "the agent did this on its own" apart from
 * "I'm driving this." A single pulse trace reads as a heartbeat without
 * leaning on emoji, matching the hand-rolled inline-SVG icon convention
 * (`IconHeartEKG`, `IconLock`).
 *
 * Sized by the parent's CSS (className). Inherits stroke colour from
 * `currentColor`.
 */
export function IconAutonomous({ className }: { className?: string }): React.JSX.Element {
  return (
    <svg
      className={className}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {/* EKG pulse trace: flat-line, spike, flat-line — the classic
          "heartbeat" silhouette, distinct from the spinning braille. */}
      <path d="M1 8 H4 L5.5 4 L7.5 12 L9.5 6.5 L10.5 8 H15" />
    </svg>
  )
}
