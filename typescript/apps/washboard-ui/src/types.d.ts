/// <reference types="vite-plugin-svgr/client" />
/// <reference types="vite/client" />
/// <reference types="vite-plugin-svgr/client" />

import {CellContext, RowData} from '@tanstack/react-table';
import * as React from 'react';

// eslint-disable-next-line @typescript-eslint/consistent-type-definitions -- intended behavior
interface ImportMeta {
  readonly env: {
    readonly VITE_NATS_WEBSOCKET_URL: string;
  };
}

declare module '@tanstack/react-table' {
  // eslint-disable-next-line @typescript-eslint/consistent-type-definitions -- intended behavior
  interface ColumnMeta<TData extends RowData, TValue> {
    baseRow: 'visible' | 'hidden' | 'empty';
    expandedRow: 'visible' | 'hidden' | 'empty';
    expandedCell?: (
      key: string,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any -- don't know what the expandedRows are from here
      expandedRows: any,
    ) => (info: CellContext<TData, TValue>) => React.ReactElement | string | number;
  }
}
