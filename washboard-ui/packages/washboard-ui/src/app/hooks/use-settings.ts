import {useContext} from 'react';
import {SettingsContext, SettingsContextValue} from '../contexts/settings-context';

export function useSettings(): SettingsContextValue {
  return useContext(SettingsContext);
}
