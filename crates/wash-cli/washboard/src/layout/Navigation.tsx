import {ReactElement} from 'react';
import SvgLogo from 'assets/logo-wide.svg?react';
import {Settings} from 'settings';
import {ConnectionStatus} from "../components/connection-status.tsx";

function Navigation(): ReactElement {
  return (
    <div className="rounded-xl bg-brand p-2 text-brand-foreground md:p-4">
      <div className="flex items-center justify-between">
        <SvgLogo />
        <div className="flex gap-2 items-center">
          <ConnectionStatus />
          <Settings />
        </div>
      </div>
    </div>
  );
}

export default Navigation;
