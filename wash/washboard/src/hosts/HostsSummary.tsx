import {formatDistanceToNow, formatDuration, intervalToDuration} from 'date-fns';
import {ReactElement} from 'react';
import useLatticeData from 'lattice/use-lattice-data';
import {Accordion, AccordionContent, AccordionItem, AccordionTrigger} from 'ui/accordion';
import {Badge} from 'ui/badge';
import {ShortCopy} from 'ui/short-copy';
import {Table, TableBody, TableCell, TableHead, TableRow} from 'ui/table';

export function HostsSummary(): ReactElement {
  const {hosts} = useLatticeData();

  const hostsArray = Object.values(hosts).sort((a, b) => (a.id > b.id ? 1 : -1));

  return (
    <div>
      <h2 className="my-4 text-lg">Hosts</h2>
      <div className="grid grid-cols-1 grid-rows-1 gap-2">
        <Accordion type="single" collapsible className="w-full">
          {hostsArray.map((host) => (
            <AccordionItem value={host.id} key={host.id}>
              <AccordionTrigger>
                <div className="me-2 flex w-full gap-2">
                  <Badge>{host.version}</Badge>
                  <span className="truncate">{host.friendly_name}</span>
                </div>
              </AccordionTrigger>
              <AccordionContent>
                <Table>
                  <TableBody>
                    <TableRow>
                      <TableHead className="p-2 align-baseline">Uptime</TableHead>
                      <TableCell>
                        {formatDuration(
                          intervalToDuration({start: 0, end: host.uptime_seconds * 1000}),
                        )}
                      </TableCell>
                    </TableRow>
                    <TableRow>
                      <TableHead className="p-2 align-baseline">Last Seen</TableHead>
                      <TableCell>
                        {formatDistanceToNow(new Date(host.last_seen), {addSuffix: true})}
                      </TableCell>
                    </TableRow>
                    <TableRow>
                      <TableHead className="p-2 align-baseline">Host ID</TableHead>
                      <TableCell>
                        <ShortCopy
                          text={host.id}
                          className="w-40 md:w-64 lg:w-auto lg:[mask-image:none]"
                        />
                      </TableCell>
                    </TableRow>
                    <TableRow>
                      <TableHead className="p-2 align-baseline">Components</TableHead>
                      <TableCell>{Object.values(host.actors).length.toString()}</TableCell>
                    </TableRow>
                    <TableRow>
                      <TableHead className="p-2 align-baseline">Providers</TableHead>
                      <TableCell>{Object.values(host.providers).length.toString()}</TableCell>
                    </TableRow>
                    <TableRow>
                      <TableHead className="p-2 align-baseline">Labels</TableHead>
                      <TableCell>
                        {Object.entries(host.labels).map(([key, value]) => (
                          <Badge key={key} variant="outline" className="me-0.5">
                            {key}={value}
                          </Badge>
                        ))}
                      </TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </AccordionContent>
            </AccordionItem>
          ))}
        </Accordion>
      </div>
    </div>
  );
}
