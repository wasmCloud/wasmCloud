import {ReactElement} from 'react';
import SvgLogo from '@/assets/logo-wide.svg?react';
import {ConnectionStatus} from '@/components/connection-status';
import {Settings} from '@/settings';

export function Navigation(): ReactElement {
  return (
    <div className="rounded-xl bg-brand p-2 text-brand-foreground md:p-4">
      <div className="flex items-center justify-between">
        <SvgLogo />
        <div className="flex items-center gap-2">
          <ConnectionStatus />
          <Settings />
        </div>
      </div>
    </div>
  );
}
