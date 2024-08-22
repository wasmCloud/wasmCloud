import {ReactElement} from 'react';
import {WadmManagedAssetOption} from '@/app/components/wadm-indicator/types';
import {useSettings} from '@/app/hooks/use-settings';
import WadmLogo from '@/assets/wadm-logo.svg?react';

const annotationManagedBy = 'wasmcloud.dev/managed-by' as const;
const annotationManagedByIdentifier = 'wadm' as const;

type ColumnValue = ReactElement | string | null;

const featureMapper: Map<WadmManagedAssetOption, [ColumnValue, ColumnValue]> = new Map([
  [WadmManagedAssetOption.None, [null, null]],
  [WadmManagedAssetOption.Logo, [<WadmLogo key={WadmManagedAssetOption.Logo} />, null]],
  [WadmManagedAssetOption.Text, ['wadm', null]],
  [WadmManagedAssetOption.TextAndDash, ['wadm', '-']],
]);

const getIndicatorBasedOnFeature = (feature: WadmManagedAssetOption) => {
  return featureMapper.get(feature) || [null, null];
};

export function WadmManagedIndicator(annotations: Record<string, string>): ColumnValue {
  const {wadmManagedAsset} = useSettings();
  const [managed, notManaged] = getIndicatorBasedOnFeature(wadmManagedAsset);
  return annotations[annotationManagedBy] === annotationManagedByIdentifier ? managed : notManaged;
}
