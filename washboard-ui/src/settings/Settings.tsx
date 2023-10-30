import {SettingsIcon} from 'lucide-react';
import {ReactElement} from 'react';
import LatticeSettings from '@/lattice/LatticeSettings';
import {Button} from '@/ui/button';
import {Label} from '@/ui/label';
import {DarkModeToggle} from './DarkModeToggle';
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from '@/ui/sheet';

function Settings(): ReactElement {
  return (
    <Sheet>
      <SheetTrigger asChild>
        <Button variant="ghost" size="icon" className="h-6 w-6 p-0.5">
          <SettingsIcon className="h-full w-full" />
          <span className="sr-only">Settings</span>
        </Button>
      </SheetTrigger>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>Settings</SheetTitle>
          <SheetDescription>
            Make changes to your settings here. Click update when you're done.
          </SheetDescription>
        </SheetHeader>
        <div className="flex flex-col gap-6">
          <div className="flex flex-col gap-3">
            <h3 className="font-semibold">Display</h3>
            <div className="grid w-full max-w-sm items-center gap-1.5">
              <Label htmlFor="dark-mode">Dark Mode</Label>
              <DarkModeToggle id="dark-mode" />
            </div>
          </div>
          <LatticeSettings />
        </div>
      </SheetContent>
    </Sheet>
  );
}

export {Settings};
