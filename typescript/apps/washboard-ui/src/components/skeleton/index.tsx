import * as React from 'react';
import {cn} from '@/helpers';

function Skeleton({className, ...props}: React.HTMLAttributes<HTMLDivElement>): React.ReactElement {
  return <div className={cn('animate-pulse rounded-md bg-primary/10', className)} {...props} />;
}

export {Skeleton};
