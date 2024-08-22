import {Home} from 'lucide-react';
import {ReactElement} from 'react';
import {RouteObject} from 'react-router-dom';
import {Dashboard} from '@/app/components/dashboard';
import {AppLayout} from '@/app/layout';

export type AppRouteObject = RouteObject & {
  handle?: {
    title?: string;
    breadcrumbTitle?: string;
    icon?: ReactElement;
    hideInMenu?: boolean;
    hideInBreadcrumb?: boolean;
  };
  children?: AppRouteObject[];
};

export const routes: RouteObject[] = [
  {
    element: <AppLayout />,
    children: [
      {
        index: true,
        path: '/',
        element: <Dashboard />,
        handle: {
          breadcrumbTitle: 'Washboard',
          title: 'Washboard',
          icon: <Home />,
        },
      },
    ],
  },
];
