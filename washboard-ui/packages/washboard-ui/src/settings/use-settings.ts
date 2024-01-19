import {useContext} from 'react';
import {SettingsContext, SettingsContextValue} from './settings-context';

export function useSettings(): SettingsContextValue {
  return useContext(SettingsContext);
}
