import {ReactElement} from 'react';
import {RouterProvider, createBrowserRouter} from 'react-router-dom';
import AppProvider from 'lib/AppProvider';
import {routes} from 'routes';
import {SettingsProvider} from 'settings/SettingsContext';

function App(): ReactElement {
  return (
    <AppProvider components={[SettingsProvider]}>
      <RouterProvider router={createBrowserRouter(routes)} />
    </AppProvider>
  );
}

export default App;
