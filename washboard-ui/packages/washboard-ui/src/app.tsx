import {ReactElement} from 'react';
import {RouterProvider, createBrowserRouter} from 'react-router-dom';
import {AppLatticeClientProvider} from '@/context/app-lattice-client-provider';
import {AppProvider} from '@/context/app-provider';
import {routes} from '@/routes';
import {SettingsProvider} from '@/settings/settings-context';

export function App(): ReactElement {
  return (
    <AppProvider components={[SettingsProvider, AppLatticeClientProvider]}>
      <RouterProvider router={createBrowserRouter(routes)} />
    </AppProvider>
  );
}
