import { useSidebarStore } from '@/stores/sidebar'

/** Collapse or open the workspaces nav. Lives at the bottom of the nav. */
export function SidebarCollapseButton(): JSX.Element {
  const collapsed = useSidebarStore((s) => s.isCollapsed)
  const toggle = useSidebarStore((s) => s.toggle)
  return (
    <button
      type="button"
      className="no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 bg-white/[0.04] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.08] transition-colors"
      onClick={() => toggle()}
      title={collapsed ? 'Open workspaces sidebar' : 'Collapse workspaces sidebar'}
      aria-label={collapsed ? 'Open workspaces sidebar' : 'Collapse workspaces sidebar'}
    >
      <svg
        className="w-4 h-4"
        viewBox="0 0 14 14"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden
      >
        {collapsed ? (
          <>
            <path d="M4.5 3.5 L8 7 L4.5 10.5" />
            <path d="M8 3.5 L11.5 7 L8 10.5" />
          </>
        ) : (
          <>
            <path d="M9.5 3.5 L6 7 L9.5 10.5" />
            <path d="M6 3.5 L2.5 7 L6 10.5" />
          </>
        )}
      </svg>
    </button>
  )
}
