import {AppRouteObject} from 'routes';

export function getBreadcrumbs(route: AppRouteObject): AppRouteObject[] {
  const breadcrumbs: AppRouteObject[] = [];
  let current: AppRouteObject | undefined = route;
  while (current) {
    if (!current.handle.hideInBreadcrumb) {
      breadcrumbs.unshift(current);
    }
    current = current.children?.[0];
  }
  return breadcrumbs;
}
