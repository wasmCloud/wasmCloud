import {useLatticeData} from '@wasmcloud/lattice-client-react';
import {ReactElement} from 'react';
import {ComponentsTable} from '@/app/components/components-table';
import {HostsSummary} from '@/app/components/hosts-summary';
import {LinksTable} from '@/app/components/links-table';
import {ProvidersTable} from '@/app/components/providers-table';
import {StatsTile} from '@/app/components/stats-tile';
import {Card, CardContent, CardHeader} from '@/components/card';
import {ConfigsTable} from './configs-table';

export function Dashboard(): ReactElement {
  const {hosts, components, providers, links} = useLatticeData();

  const hostsCount = Object.keys(hosts).length.toString();
  const componentsCount = Object.keys(components).length.toString();
  const providersCount = Object.keys(providers).length.toString();
  const linksCount = Object.keys(links).length.toString();

  return (
    <div className="flex flex-col gap-2 md:gap-4">
      <div className="grid grid-cols-1 grid-rows-1 gap-2 sm:grid-cols-12">
        <div className="col-span-6 sm:col-span-5 md:col-span-4 lg:col-span-3 xl:col-span-4">
          <div className=" rounded-xl bg-seafoam-700 p-4 text-seafoam-100 dark:bg-seafoam-100 dark:text-seafoam-700 ">
            <h2 className="mb-4 text-lg">Overview</h2>
            <div className="grid grid-cols-1 grid-rows-1 gap-2 xl:grid-cols-2">
              <StatsTile title="Hosts" value={hostsCount} />
              <StatsTile title="Components" value={componentsCount} />
              <StatsTile title="Providers" value={providersCount} />
              <StatsTile title="Links" value={linksCount} />
            </div>
          </div>
        </div>
        <div className="col-span-6 p-4 sm:col-span-7 md:col-span-8 lg:col-span-9 xl:col-span-8">
          <HostsSummary />
        </div>
      </div>
      <Card variant="accent" className="w-full rounded-xl">
        <CardHeader>Components</CardHeader>
        <CardContent>
          <ComponentsTable />
        </CardContent>
      </Card>
      <Card variant="accent" className="w-full rounded-xl">
        <CardHeader>Providers</CardHeader>
        <CardContent>
          <ProvidersTable />
        </CardContent>
      </Card>
      <Card variant="accent" className="w-full rounded-xl">
        <CardHeader>Links</CardHeader>
        <CardContent>
          <LinksTable />
        </CardContent>
      </Card>
      <Card variant="accent" className="w-full rounded-xl">
        <CardHeader>Configs</CardHeader>
        <CardContent>
          <ConfigsTable />
        </CardContent>
      </Card>
    </div>
  );
}
