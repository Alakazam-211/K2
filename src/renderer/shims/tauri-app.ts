/**
 * Web shim for `@tauri-apps/api/app`.
 */

function appVersion(): string {
  const env = (import.meta as ImportMeta & { env?: Record<string, string> }).env
  return env?.VITE_APP_VERSION ?? env?.VITE_K2_VERSION ?? '0.0.0-web'
}

export async function getVersion(): Promise<string> {
  return appVersion()
}

export async function getName(): Promise<string> {
  return 'K2'
}

export async function getTauriVersion(): Promise<string> {
  return 'web'
}

export async function show(): Promise<void> {}
export async function hide(): Promise<void> {}
