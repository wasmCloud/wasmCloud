import {createContext} from 'react';
import {WadmManagedAssetOption} from '@/app/components/wadm-indicator/types';

export enum DarkModeOption {
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
