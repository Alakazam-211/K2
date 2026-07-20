/**
 * True when the renderer was built with the hosted web Vite variant
 * (`vite.config.web.ts` defines `import.meta.env.VITE_WEB`).
 *
 * Desktop / Tauri builds leave this unset — keep all local-daemon paths
 * byte-identical when false.
 */
export function isWebClient(): boolean {
  const v = import.meta.env.VITE_WEB
  return v === true || v === 'true'
}
