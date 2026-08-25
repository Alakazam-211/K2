// Air-gap hide-UI signal from the connected daemon's /boot-status.
// Daemon refuse is the authority; this only hides Tunnel / skips updater.

export const AIRGAP_TEACHING =
  'Air-gap is on (K2_AIRGAP=1). This app will not phone Connect, GitHub, or other hosted services.'

let connectedAirgap = false

/** Record the connected daemon's `airgap.enabled` from /boot-status. */
export function setConnectedAirgap(enabled: boolean): void {
  connectedAirgap = enabled
}

/** True when the connected daemon advertised air-gap (hide-UI only). */
export function isAirgap(): boolean {
  return connectedAirgap
}
