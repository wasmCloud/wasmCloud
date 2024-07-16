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
import {useLatticeData, WasmCloudProvider} from '@wasmcloud/lattice-client-react';
import {ChevronDown, ChevronRight} from 'lucide-react';
import {Fragment, ReactElement, useMemo, useState} from 'react';
import {Collapsible, CollapsibleContent, CollapsibleTrigger} from '@/components/collapsible';
import {ShortCopy} from '@/components/short-copy';
import {StatusIndicator} from '@/components/status-indicator';
import {Table, TableBody, TableCell, TableHead, TableHeader, TableRow} from '@/components/table';
import {useColumnVisibility, hideableWadmManagedColumnId} from '../hooks/use-column-visibility';
import {WadmManagedIndicator} from './wadm-indicator/wadm-indicator';

const columnHelper = createColumnHelper<WasmCloudProvider>();

const columns = [
  columnHelper.display({
    id: 'expand',
    cell: () => {
      return (
        <CollapsibleTrigger className="flex place-items-center" asChild>
          {(open) =>
            open ? <ChevronDown className="size-4" /> : <ChevronRight className="size-4" />
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
    id: 'name',
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
  columnHelper.accessor('annotations', {
    id: hideableWadmManagedColumnId,
    header: 'Managed',
    enableHiding: true,
    cell: (info) => WadmManagedIndicator(info.getValue()),
  }),
];

export function ProvidersTable(): ReactElement {
  const {providers} = useLatticeData();

  const data = useMemo(
    () => Object.values(providers).sort((a, b) => (a.id > b.id ? 1 : -1)),
    [providers],
  );

  const [sorting, setSorting] = useState<SortingState>([]);
  const {columnVisibility, setColumnVisibility} = useColumnVisibility(columns);

  const table = useReactTable({
    data,
    columns,
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getPaginationRowModel: getPaginationRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    state: {sorting, columnVisibility},
    onColumnVisibilityChange: setColumnVisibility,
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
                          {(row.getValue('hosts') as string[]).length > 0 &&
                            (row.getValue('hosts') as string[])
                              .sort((a, b) => (a > b ? 1 : -1))
                              .map((hostId) => (
                                <TableRow key={row.id + '-' + hostId} data-expanded="true">
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
                                                  hostId,
                                                  hostId,
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
