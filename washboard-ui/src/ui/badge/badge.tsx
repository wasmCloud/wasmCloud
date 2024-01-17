import {type VariantProps} from 'class-variance-authority';
import * as React from 'react';

import {cn} from '@/lib/utils';
import {badgeVariants} from './variants';

export type BadgeProps = React.HTMLAttributes<HTMLDivElement> & VariantProps<typeof badgeVariants>;

export function Badge({className, variant, ...props}: BadgeProps): React.ReactElement {
  return <div className={cn(badgeVariants({variant}), className)} {...props} />;
}
