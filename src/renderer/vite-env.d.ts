/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_WEB?: boolean | string
  readonly VITE_APP_VERSION?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
