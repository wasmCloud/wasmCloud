import {PropsWithChildren, ReactElement, useEffect} from 'react';
import {useLocalStorage} from 'usehooks-ts';

import {WadmManagedAssetOption} from '../components/wadm-indicator/types';
import {DarkModeOption, SettingsContext} from './settings-context';

export function SettingsProvider({children}: PropsWithChildren): ReactElement {
  const [darkMode, setDarkMode] = useLocalStorage('theme', DarkModeOption.System);

  const [wadmManagedAsset, setWadmManagedAsset] = useLocalStorage(
    'wadmManagedAsset',
    WadmManagedAssetOption.Logo,
  );

  useEffect(() => {
    if (
      darkMode === DarkModeOption.Dark ||
      (darkMode === DarkModeOption.System &&
        globalThis.matchMedia('(prefers-color-scheme: dark)').matches)
    ) {
      document.documentElement.classList.add('dark');
    } else {
      document.documentElement.classList.remove('dark');
    }
  }, [darkMode]);

  return (
    <SettingsContext.Provider
      value={{darkMode, setDarkMode, wadmManagedAsset, setWadmManagedAsset}}
    >
      {children}
    </SettingsContext.Provider>
  );
}
