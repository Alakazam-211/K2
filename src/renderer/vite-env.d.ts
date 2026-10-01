/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_WEB?: boolean | string
  readonly VITE_APP_VERSION?: string
  /** Dev-only room-frame IPC probe (Home P1.5 spike). */
  readonly VITE_K2_ROOMFRAME_PROBE?: string
  /** Dev-only: POST the room-frame probe results to this URL. */
  readonly VITE_K2_ROOMFRAME_SINK?: string
  /** Dev-only: ConnectHost JSON for a `#room=<id>` probe frame (temp daemons only). */
  readonly VITE_K2_ROOMFRAME_HOST?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
