import {CellContext, RowData} from '@tanstack/react-table';
import * as React from 'react';

declare module '@tanstack/table-core' {
  interface ColumnMeta<TData extends RowData, TValue> {
    baseRow: 'visible' | 'hidden' | 'empty';
    expandedRow: 'visible' | 'hidden' | 'empty';
    expandedCell?: (
      key: string,
      expandedRows: any,
    ) => (info: CellContext<TData, TValue>) => React.ReactElement | string | number;
  }
}
