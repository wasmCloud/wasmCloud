import {ReactElement} from 'react';
import {Outlet} from 'react-router';
import Navigation from './Navigation';

function AppLayout(): ReactElement {
  return (
    <div className="mx-auto flex w-full max-w-7xl flex-col gap-2 p-2 md:gap-4 md:p-6">
      <Navigation />
      <div className="flex flex-col">
        <Outlet />
      </div>
    </div>
  );
}

export default AppLayout;
