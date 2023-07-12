import {
  SortingState,
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  getFilteredRowModel,
  getPaginationRowModel,
  getSortedRowModel,
  useReactTable,
} from '@tanstack/react-table';
import {ChevronDown, ChevronRight} from 'lucide-react';
import {Fragment, ReactElement, useState} from 'react';
import {WadmActor} from 'lattice/lattice-service';
import useLatticeData from 'lattice/use-lattice-data';
import {Collapsible, CollapsibleContent, CollapsibleTrigger} from 'ui/collapsible';
import {ShortCopy} from 'ui/short-copy';
import {Table, TableHeader, TableRow, TableHead, TableBody, TableCell} from 'ui/table';
import {countInstances} from './count-instances';

const columnHelper = createColumnHelper<WadmActor>();

const columns = [
  columnHelper.display({
    id: 'expand',
    cell: () => {
      return (
        <CollapsibleTrigger className="flex place-items-center" asChild>
          {(open) =>
            open ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />
          }
        </CollapsibleTrigger>
      );
    },
    meta: {
      baseRow: 'visible',
      expandedRow: 'empty',
    },
  }),
  columnHelper.accessor('name', {
    header: 'Name',
    meta: {
      baseRow: 'visible',
      expandedRow: 'empty',
    },
  }),
  // columnHelper.accessor('capabilities', {
  //   header: 'Capabilities',
  //   cell: (info) => {
  //     const capabilities = [...info.getValue()].sort((a, b) => (a > b ? 1 : -1));
  //     return (
  //       <div className="flex gap-1">
  //         {capabilities.map((cap) => (
  //           <Badge key={cap} variant="outline">
  //             {cap}
  //           </Badge>
  //         ))}
  //       </div>
  //     );
  //   },
  //   meta: {
  //     baseRow: 'visible',
  //     expandedRow: 'empty',
  //   },
  // }),
  columnHelper.accessor('instances', {
    header: 'Hosts',
    id: 'hosts',
    cell: (info) => Object.keys(info.getValue()).length.toString(),
    meta: {
      baseRow: 'visible',
      expandedRow: 'hidden',
    },
  }),
  columnHelper.accessor('instances', {
    header: 'Hosts',
    id: 'hosts-expanded',
    cell: (info) => Object.keys(info.getValue()).length.toString(),
    meta: {
      baseRow: 'hidden',
      expandedRow: 'visible',
      expandedCell: (hostId: string) => () => {
        return <ShortCopy text={hostId} />;
      },
    },
  }),
  columnHelper.accessor('instances', {
    header: 'Count',
    id: 'count',
    cell: (info) => countInstances(info.getValue()),
    meta: {
      baseRow: 'visible',
      expandedRow: 'hidden',
    },
  }),
  columnHelper.accessor('instances', {
    header: 'Count',
    id: 'count-expanded',
    cell: (info) => countInstances(info.getValue()),
    meta: {
      baseRow: 'hidden',
      expandedRow: 'visible',
      expandedCell: (_hostId: string, instances: string) => () => {
        return instances.length;
      },
    },
  }),
  columnHelper.accessor('id', {
    header: 'ID',
    cell: (info) => <ShortCopy text={info.getValue()} />,
    meta: {
      baseRow: 'visible',
      expandedRow: 'empty',
    },
  }),
];

function ActorsTable(): ReactElement {
  const {actors} = useLatticeData();

  const data = Object.values(actors);

  const [sorting, setSorting] = useState<SortingState>([]);

  const table = useReactTable({
    data,
    columns,
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getPaginationRowModel: getPaginationRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    state: {sorting},
  });

  return (
    <div>
      <div className="w-full">
        <div className="rounded-md border bg-primary-foreground">
          <Table>
            <TableHeader>
              {table.getHeaderGroups().map((headerGroup) => (
                <TableRow key={headerGroup.id}>
                  {headerGroup.headers.map((header) => {
                    return header.column.columnDef.meta?.baseRow === 'hidden' ? null : (
                      <TableHead
                        key={header.id}
                        className="bg-seafoam-700 text-seafoam-100 first:rounded-tl-sm last:rounded-tr-sm"
                      >
                        {header.isPlaceholder
                          ? null
                          : flexRender(header.column.columnDef.header, header.getContext())}
                      </TableHead>
                    );
                  })}
                </TableRow>
              ))}
            </TableHeader>
            <TableBody>
              {table.getRowModel().rows?.length ? (
                table.getRowModel().rows.map((row) => (
                  <Collapsible key={row.id} asChild>
                    <Fragment>
                      <TableRow>
                        {row
                          .getVisibleCells()
                          .map((cell) =>
                            cell.column.columnDef.meta?.baseRow === 'hidden' ? null : (
                              <TableCell key={cell.id}>
                                {cell.column.columnDef.meta?.baseRow === 'empty'
                                  ? null
                                  : flexRender(cell.column.columnDef.cell, cell.getContext())}
                              </TableCell>
                            ),
                          )}
                      </TableRow>
                      <CollapsibleContent asChild>
                        <Fragment>
                          {Object.keys(row.getValue('hosts-expanded')).length > 0 &&
                            Object.entries(
                              row.getValue('hosts-expanded') as Record<string, string[]>,
                            )
                              .sort((a, b) => (a[0] > b[0] ? 1 : -1))
                              .map(([host, instances]) => (
                                <TableRow key={row.id + '-' + host} data-expanded="true">
                                  {row
                                    .getVisibleCells()
                                    .map((cell) =>
                                      cell.column.columnDef.meta?.expandedRow ===
                                      'hidden' ? null : (
                                        <TableCell key={cell.id}>
                                          {cell.column.columnDef.meta?.expandedRow === 'empty'
                                            ? null
                                            : flexRender(
                                                cell.column.columnDef.meta?.expandedCell?.(
                                                  host,
                                                  instances,
                                                ) ?? cell.column.columnDef.cell,
                                                cell.getContext(),
                                              )}
                                        </TableCell>
                                      ),
                                    )}
                                </TableRow>
                              ))}
                        </Fragment>
                      </CollapsibleContent>
                    </Fragment>
                  </Collapsible>
                ))
              ) : (
                <TableRow>
                  <TableCell colSpan={columns.length} className="h-24 text-center">
                    No results.
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </div>
      </div>
    </div>
  );
}

export default ActorsTable;
