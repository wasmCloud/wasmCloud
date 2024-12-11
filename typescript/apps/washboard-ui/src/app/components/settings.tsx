import {SettingsIcon} from 'lucide-react';
import {PropsWithChildren, ReactElement} from 'react';
import {DarkModeToggle} from '@/app/components/dark-mode-toggle';
import {LatticeSettings} from '@/app/components/lattice-settings';
import {Button} from '@/components/button';
import {FormItem} from '@/components/form';
import {Label} from '@/components/label';
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from '@/components/sheet';
import {WadmManagedToggle} from './wadm-indicator/wadm-managed-toggle';

function Settings(): ReactElement {
  return (
    <Sheet>
      <SheetTrigger asChild>
        <Button variant="ghost" size="icon" className="size-6 p-0.5">
          <SettingsIcon className="size-full" />
          <span className="sr-only">Settings</span>
        </Button>
      </SheetTrigger>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>Settings</SheetTitle>
          <SheetDescription>
            Make changes to your settings here. Click update when you&nbsp;re done.
          </SheetDescription>
        </SheetHeader>
        <div className="my-4 grid gap-8">
          <SettingsSection>
            <SettingsSectionLabel>Display</SettingsSectionLabel>
            <SettingsSectionContent>
              <FormItem>
                <Label htmlFor="dark-mode">Theme</Label>
                <DarkModeToggle id="dark-mode" />
              </FormItem>
              <FormItem>
                <Label htmlFor="wadm-managed-indicator">WADM managed assets indicator</Label>
                <WadmManagedToggle id="wadm-managed-indicator" />
              </FormItem>
            </SettingsSectionContent>
          </SettingsSection>
          <SettingsSection>
            <SettingsSectionLabel>Lattice Configuration</SettingsSectionLabel>
            <SettingsSectionContent>
              <LatticeSettings />
            </SettingsSectionContent>
          </SettingsSection>
        </div>
      </SheetContent>
    </Sheet>
  );
}

function SettingsSection({children}: PropsWithChildren): ReactElement {
  return <div className="mb-6">{children}</div>;
}

function SettingsSectionLabel({children}: PropsWithChildren): ReactElement {
  return <div className="mb-3 font-semibold">{children}</div>;
}

function SettingsSectionContent({children}: PropsWithChildren): ReactElement {
  return <div className="grid gap-4">{children}</div>;
}

export {Settings};
