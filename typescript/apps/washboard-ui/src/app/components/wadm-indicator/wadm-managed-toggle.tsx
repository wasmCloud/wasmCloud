import * as SelectPrimitive from '@radix-ui/react-select';
import {ComponentPropsWithoutRef, ElementRef, forwardRef} from 'react';
import {WadmManagedAssetOption} from '@/app/components/wadm-indicator/types';
import {useSettings} from '@/app/hooks/use-settings';
import {Select, SelectContent, SelectItem, SelectTrigger, SelectValue} from '@/components/select';

const WadmManagedToggle = forwardRef<
  ElementRef<typeof SelectPrimitive.Trigger>,
  ComponentPropsWithoutRef<typeof SelectPrimitive.Trigger>
>((props, ref) => {
  const {wadmManagedAsset, setWadmManagedAsset} = useSettings();

  return (
    <Select onValueChange={setWadmManagedAsset} value={wadmManagedAsset}>
      <SelectTrigger ref={ref} className="w-full" {...props}>
        <SelectValue placeholder="How to show managed indicator" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={WadmManagedAssetOption.Logo}>Logo</SelectItem>
        <SelectItem value={WadmManagedAssetOption.None}>None</SelectItem>
        <SelectItem value={WadmManagedAssetOption.Text}>Text</SelectItem>
        <SelectItem value={WadmManagedAssetOption.TextAndDash}>Text And Dash</SelectItem>
      </SelectContent>
    </Select>
  );
});

WadmManagedToggle.displayName = 'WadmManagedToggle';

export {WadmManagedToggle};
