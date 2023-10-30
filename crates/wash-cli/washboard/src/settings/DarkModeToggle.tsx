import * as SelectPrimitive from '@radix-ui/react-select';
import {ComponentPropsWithoutRef, ElementRef, forwardRef} from 'react';
import {Select, SelectContent, SelectItem, SelectTrigger, SelectValue} from 'ui/select';
import {useSettings} from './use-settings';

const DarkModeToggle = forwardRef<
  ElementRef<typeof SelectPrimitive.Trigger>,
  ComponentPropsWithoutRef<typeof SelectPrimitive.Trigger>
>((props, ref) => {
  const {darkMode, setDarkMode} = useSettings();

  return (
    <Select onValueChange={setDarkMode} value={darkMode}>
      <SelectTrigger ref={ref} className="w-full" {...props}>
        <SelectValue placeholder="Select a fruit" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="dark">Dark</SelectItem>
        <SelectItem value="light">Light</SelectItem>
        <SelectItem value="system">Follow System</SelectItem>
      </SelectContent>
    </Select>
  );
});

DarkModeToggle.displayName = 'DarkModeToggle';

export {DarkModeToggle};
