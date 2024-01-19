/// <reference types="vite/client" />
/// <reference types="vite-plugin-svgr/client" />

// eslint-disable-next-line @typescript-eslint/consistent-type-definitions -- intended behavior
interface ImportMeta {
  readonly env: {
    readonly VITE_NATS_WEBSOCKET_URL: string;
  };
}
