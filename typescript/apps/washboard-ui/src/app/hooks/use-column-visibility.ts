import {useEffect, useState} from 'react';
import {WadmManagedAssetOption} from '../components/wadm-indicator/types';
import {useSettings} from './use-settings';

export const hideableWadmManagedColumnId = 'wadm-managed' as const;

type ColumnVisibilitySettings = {[key: string]: boolean};
type Column = {
  id?: string;
};

const useColumnVisibility = <T extends Column>(columns: T[]) => {
  const {wadmManagedAsset} = useSettings();

  const [columnVisibility, setColumnVisibility] = useState<ColumnVisibilitySettings>(
    columns.reduce((accumulator, column) => {
      if (typeof column.id === 'string') {
        return Object.assign(accumulator, {[column.id]: column.id !== hideableWadmManagedColumnId});
      }
      return accumulator;
    }, {}),
  );

  useEffect(() => {
    setColumnVisibility((columnVisibility) => ({
      ...columnVisibility,
      [hideableWadmManagedColumnId]: wadmManagedAsset !== WadmManagedAssetOption.None,
    }));
  }, [wadmManagedAsset]);

  return {columnVisibility, setColumnVisibility};
};
export {useColumnVisibility};
