import {PropsWithChildren, ReactElement, createContext, useEffect} from 'react';
import {useLocalStorage} from 'usehooks-ts';
import {WadmManagedAssetOption} from '@/app/components/wadm-indicator/types';

enum DarkModeOption {
  Dark = 'dark',
  Light = 'light',
  System = 'system',
}

export type SettingsContextValue = {
  darkMode: DarkModeOption;
  setDarkMode: (darkMode: DarkModeOption) => void;
  wadmManagedAsset: WadmManagedAssetOption;
  setWadmManagedAsset: (wadmManagedAsset: WadmManagedAssetOption) => void;
};

export const SettingsContext = createContext<SettingsContextValue>({
  darkMode: DarkModeOption.System,
  setDarkMode: () => null,
  wadmManagedAsset: WadmManagedAssetOption.Logo,
  setWadmManagedAsset: () => null,
});

export function SettingsProvider({children}: PropsWithChildren): ReactElement {
  const [darkMode, setDarkMode] = useLocalStorage('theme', DarkModeOption.System);

  const [wadmManagedAsset, setWadmManagedAsset] = useLocalStorage(
    'wadmManagedAsset',
    WadmManagedAssetOption.Logo,
  );

  // sync state with localStorage
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
