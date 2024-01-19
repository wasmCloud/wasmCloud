import {ReactElement} from 'react';
import {RouterProvider, createBrowserRouter} from 'react-router-dom';
import {AppLatticeServiceProvider} from '@/context/app-lattice-service-provider';
import {AppProvider} from '@/context/app-provider';
import {routes} from '@/routes';
import {SettingsProvider} from '@/settings/settings-context';

export function App(): ReactElement {
  return (
    <AppProvider components={[SettingsProvider, AppLatticeServiceProvider]}>
      <RouterProvider router={createBrowserRouter(routes)} />
    </AppProvider>
  );
}
