import './styles/index.css';
import {ReactElement} from 'react';
import {RouterProvider, createBrowserRouter} from 'react-router-dom';
import {AppLatticeClientProvider} from '@/app/components/app-lattice-client-provider';
import {AppProvider} from '@/app/components/app-provider';
import {SettingsProvider} from '@/app/contexts/settings-context-provider';
import {routes} from '@/app/routes';

export function App(): ReactElement {
  return (
    <AppProvider components={[SettingsProvider, AppLatticeClientProvider]}>
      <RouterProvider router={createBrowserRouter(routes)} />
    </AppProvider>
  );
}
