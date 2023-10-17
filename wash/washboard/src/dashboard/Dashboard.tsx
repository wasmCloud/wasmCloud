import {ReactElement} from 'react';
import ActorsTable from 'actors/ActorsTable';
import {HostsSummary} from 'hosts';
import useLatticeData from 'lattice/use-lattice-data';
import LinksTable from 'links/LinksTable';
import ProvidersTable from 'providers/ProvidersTable';
import {Card, CardContent, CardHeader} from 'ui/card';
import StatsTile from './StatsTile';

function Dashboard(): ReactElement {
  const {hosts, actors, providers, links} = useLatticeData();

  const hostsCount = Object.keys(hosts).length.toString();
  const actorsCount = Object.keys(actors).length.toString();
  const providersCount = Object.keys(providers).length.toString();
  const linksCount = links.length.toString();

  return (
    <div className="flex flex-col gap-2 md:gap-4">
      <div className="grid grid-cols-1 grid-rows-1 gap-2 sm:grid-cols-12">
        <div className="col-span-6 sm:col-span-5 md:col-span-4 lg:col-span-3 xl:col-span-4">
          <div className=" rounded-xl bg-seafoam-700 p-4 text-seafoam-100 dark:bg-seafoam-100 dark:text-seafoam-700 ">
            <h2 className="mb-4 text-lg">Overview</h2>
            <div className="grid grid-cols-1 grid-rows-1 gap-2 xl:grid-cols-2">
              <StatsTile title="Hosts" value={hostsCount} />
              <StatsTile title="Components" value={actorsCount} />
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
          <ActorsTable />
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
    </div>
  );
}

export default Dashboard;
