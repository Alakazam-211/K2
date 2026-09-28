// Linux in-flow squares. Same paint as macOS square lights in
// traffic_lights.rs (active_rgb / inactive_rgb at alpha 0.2). Not
// AppKit, and not set_traffic_light_inset (no-op off macOS).

export const LINUX_STOPLIGHT_ROLES = ['close', 'minimize', 'maximize'] as const

export type LinuxStoplightRole = (typeof LINUX_STOPLIGHT_ROLES)[number]

/** Hit target is the macOS button frame. The painted square is 2px smaller. */
export const LINUX_STOPLIGHT_HIT_PX = 14
export const LINUX_STOPLIGHT_SQUARE_PX = 12
/** Gap between the 14px frames (origins 9 / 32 / 55). */
export const LINUX_STOPLIGHT_GAP_PX = 9
export const LINUX_STOPLIGHT_RADIUS_PX = 0

export const LINUX_STOPLIGHT_ACTIVE: Record<LinuxStoplightRole, string> = {
  close: '#FF5F57',
  minimize: '#FEBC2E',
  maximize: '#28C840',
}

/** inactive_rgb. Not the active hexes, and not one grey for both schemes. */
export const LINUX_STOPLIGHT_INACTIVE_RGB = {
  dark: { r: 0.3, g: 0.3, b: 0.3 },
  light: { r: 0.84, g: 0.84, b: 0.84 },
} as const

export const LINUX_STOPLIGHT_INACTIVE_ALPHA = 0.2

export function linuxStoplightFill(
  role: LinuxStoplightRole,
  state: { focused: boolean; dark: boolean },
): string {
  if (state.focused) return LINUX_STOPLIGHT_ACTIVE[role]
  const rgb = state.dark
    ? LINUX_STOPLIGHT_INACTIVE_RGB.dark
    : LINUX_STOPLIGHT_INACTIVE_RGB.light
  return `color(srgb ${rgb.r} ${rgb.g} ${rgb.b} / ${LINUX_STOPLIGHT_INACTIVE_ALPHA})`
}
