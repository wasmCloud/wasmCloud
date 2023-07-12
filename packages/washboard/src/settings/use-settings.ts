import {useContext} from 'react';
import {SettingsContext, SettingsContextValue} from './SettingsContext';

export function useSettings(): SettingsContextValue {
  return useContext(SettingsContext);
}
