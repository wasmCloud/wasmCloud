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
import {WadmProvider} from 'lattice/lattice-service';
import useLatticeData from 'lattice/use-lattice-data';
import {Collapsible, CollapsibleContent, CollapsibleTrigger} from 'ui/collapsible';
import {ShortCopy} from 'ui/short-copy';
import {StatusIndicator} from 'ui/status-indicator';
import {Table, TableBody, TableCell, TableHead, TableHeader, TableRow} from 'ui/table';

const columnHelper = createColumnHelper<WadmProvider>();

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
  columnHelper.accessor('contract_id', {
    header: 'Contract ID',
    meta: {
      baseRow: 'visible',
      expandedRow: 'empty',
    },
  }),
  columnHelper.accessor('link_name', {
    header: 'Link Name',
    meta: {
      baseRow: 'visible',
      expandedRow: 'empty',
    },
  }),
  columnHelper.accessor('hosts', {
    id: 'hosts',
    header: 'Hosts',
    cell: (info) => Object.keys(info.getValue()).length,
    meta: {
      baseRow: 'visible',
      expandedRow: 'hidden',
    },
  }),
  columnHelper.accessor('hosts', {
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
  columnHelper.accessor('hosts', {
    id: 'health',
    header: 'Health',
    cell: (info) => {
      const healthSummary: 'Running' | 'Pending' | 'Failed' = Object.values(info.getValue()).reduce(
        (summary, currentStatus): 'Running' | 'Pending' | 'Failed' => {
          // health status can be 'Running', 'Pending' or 'Failed'
          if (summary === 'Failed') {
            return summary;
          } else if (summary === 'Pending') {
            return currentStatus === 'Failed' ? 'Failed' : 'Pending';
          } else if (currentStatus === 'Running') {
            return currentStatus;
          } else {
            return 'Pending';
          }
        },
        'Running',
      ) as 'Running' | 'Pending' | 'Failed';
      return (
        <div className="flex place-items-center">
          <StatusIndicator status={healthSummary} className="me-2" /> {healthSummary}
        </div>
      );
    },
    meta: {
      baseRow: 'visible',
      expandedRow: 'hidden',
    },
  }),
  columnHelper.accessor('hosts', {
    id: 'health-detail',
    header: 'Health',
    cell: () => '',
    meta: {
      baseRow: 'hidden',
      expandedRow: 'visible',
      expandedCell: (_host, status: string) => () => {
        return status;
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

function ProvidersTable(): ReactElement {
  const {providers} = useLatticeData();

  const data = Object.values(providers).sort((a, b) =>
    a.id > b.id || a.link_name > b.link_name ? 1 : -1,
  );

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
                          {Object.keys(row.getValue('hosts')).length > 0 &&
                            Object.entries(row.getValue('hosts') as Record<string, string[]>)
                              .sort((a, b) => (a > b ? 1 : -1))
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

export default ProvidersTable;
