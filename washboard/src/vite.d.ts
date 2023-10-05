/// <reference types="vite/client" />
/// <reference types="vite-plugin-svgr/client" />

interface ImportMetaEnv {
  readonly VITE_NATS_WEBSOCKET_URL: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
