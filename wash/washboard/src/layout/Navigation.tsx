import {ReactElement} from 'react';
import SvgLogo from 'assets/logo-wide.svg?react';
import {Settings} from 'settings';

function Navigation(): ReactElement {
  return (
    <div className="rounded-xl bg-brand p-2 text-brand-foreground md:p-4">
      <div className="flex items-center justify-between">
        <div className="">
          <SvgLogo />
        </div>
        <Settings />
      </div>
    </div>
  );
}

export default Navigation;
