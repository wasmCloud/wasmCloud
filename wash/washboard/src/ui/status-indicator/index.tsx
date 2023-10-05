import {cva} from 'class-variance-authority';
import * as React from 'react';
import {cn} from 'lib/utils';

interface StatusIndicatorProps extends React.HTMLAttributes<HTMLDivElement> {
  status?: 'Running' | 'Pending' | 'Failed';
}

const styles = cva('inline-block h-2 w-2 rounded-full bg-current', {
  variants: {
    status: {
      Running: 'text-green-500',
      Pending: 'text-yellow-500',
      Failed: 'text-red-500',
    },
  },
});

const StatusIndicator = React.forwardRef<HTMLDivElement, StatusIndicatorProps>(
  ({className, status, ...props}, ref) => {
    return (
      <div ref={ref} className={cn(styles({status}), className)} {...props} role="presentation" />
    );
  },
);
StatusIndicator.displayName = 'StatusIndicator';

export {StatusIndicator};
