import {ReactElement} from 'react';
import {RouterProvider, createBrowserRouter} from 'react-router-dom';
import {AppLatticeServiceProvider} from '@/context/AppLatticeServiceProvider';
import {AppProvider} from '@/context/AppProvider';
import {routes} from '@/routes';
import {SettingsProvider} from '@/settings/SettingsContext';

function App(): ReactElement {
  return (
    <AppProvider components={[SettingsProvider, AppLatticeServiceProvider]}>
      <RouterProvider router={createBrowserRouter(routes)} />
    </AppProvider>
  );
}

export default App;
