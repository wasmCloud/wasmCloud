import {PropsWithChildren, ReactElement, createContext, useEffect} from 'react';
import {useLocalStorage} from 'usehooks-ts';

enum DarkModeOption {
  Dark = 'dark',
  Light = 'light',
  System = 'system',
}

export interface SettingsContextValue {
  darkMode: DarkModeOption;
  setDarkMode: (darkMode: DarkModeOption) => void;
}

export const SettingsContext = createContext<SettingsContextValue>({
  darkMode: DarkModeOption.System,
  setDarkMode: () => null,
});

export function SettingsProvider({children}: PropsWithChildren): ReactElement {
  const [darkMode, setDarkMode] = useLocalStorage('theme', DarkModeOption.System);

  // sync state with localStorage
  useEffect(() => {
    if (
      darkMode === DarkModeOption.Dark ||
      (darkMode === DarkModeOption.System &&
        window.matchMedia('(prefers-color-scheme: dark)').matches)
    ) {
      document.documentElement.classList.add('dark');
    } else {
      document.documentElement.classList.remove('dark');
    }
  }, [darkMode]);

  return (
    <SettingsContext.Provider value={{darkMode, setDarkMode}}>{children}</SettingsContext.Provider>
  );
}
