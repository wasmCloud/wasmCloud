import {SettingsIcon} from 'lucide-react';
import {PropsWithChildren, ReactElement, useState} from 'react';
import LatticeSettings from 'lattice/LatticeSettings';
import {Button} from 'ui/button';
import {Label} from 'ui/label';
import {Popover, PopoverContent, PopoverTrigger} from 'ui/popover';
import {DarkModeToggle} from './DarkModeToggle';

function Settings(): ReactElement {
  const [open, setOpen] = useState<boolean>(false);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild onClick={(): void => setOpen(!open)}>
        <Button variant="ghost" size={'icon'} className="h-6 w-6 p-0.5">
          <SettingsIcon className="h-full w-full" />
          <div className="sr-only">Settings</div>
        </Button>
      </PopoverTrigger>
      <PopoverContent side="bottom" align="end">
        <SettingsSection>
          <SettingsSectionLabel>Display</SettingsSectionLabel>
          <div className="grid w-full max-w-sm items-center gap-1.5">
            <Label htmlFor="dark-mode">Dark Mode</Label>
            <DarkModeToggle id="dark-mode" />
          </div>
        </SettingsSection>

        <SettingsSectionLabel>Lattice Configuration</SettingsSectionLabel>
        <LatticeSettings onSave={(): void => setOpen(false)} />
      </PopoverContent>
    </Popover>
  );
}

function SettingsSection({children}: PropsWithChildren): ReactElement {
  return <div className="mb-6">{children}</div>;
}

function SettingsSectionLabel({children}: PropsWithChildren): ReactElement {
  return <div className="mb-3 font-semibold">{children}</div>;
}

export {Settings};
