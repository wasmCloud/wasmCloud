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
import {WasmCloudComponent, useLatticeData} from '@wasmcloud/lattice-client-react';
import {ChevronDown, ChevronRight} from 'lucide-react';
import {Fragment, ReactElement, useMemo, useState} from 'react';
import {Collapsible, CollapsibleContent, CollapsibleTrigger} from '@/components/collapsible';
import {ShortCopy} from '@/components/short-copy';
import {Table, TableHeader, TableRow, TableHead, TableBody, TableCell} from '@/components/table';
import {useColumnVisibility, hideableWadmManagedColumnId} from '../hooks/use-column-visibility';
import {WadmManagedIndicator} from './wadm-indicator/wadm-indicator';

const columnHelper = createColumnHelper<WasmCloudComponent>();

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
    id: 'name',
    header: 'Name',
    meta: {
      baseRow: 'visible',
      expandedRow: 'empty',
    },
  }),

  columnHelper.accessor('instances', {
    header: 'Hosts',
    id: 'instances',
    cell: (info) => Object.keys(info.getValue()).length,
    meta: {
      baseRow: 'visible',
      expandedRow: 'hidden',
    },
  }),
  columnHelper.accessor('instances', {
    header: 'Hosts',
    id: 'instances-expanded',
    meta: {
      baseRow: 'hidden',
      expandedRow: 'visible',
      expandedCell: (_index, instances: string) => () => {
        return <ShortCopy text={instances} />;
      },
    },
  }),
  columnHelper.accessor('id', {
    id: 'id',
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
    cell: (info) => WadmManagedIndicator(info.getValue()),
  }),
];

export function ComponentsTable(): ReactElement {
  const {components} = useLatticeData();

  const data = useMemo(() => Object.values(components), [components]);
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
                          {Object.keys(row.getValue('instances-expanded')).length > 0 &&
                            Object.entries(
                              row.getValue('instances-expanded') as Record<string, string[]>,
                            )
                              .sort((a, b) => (a[0] > b[0] ? 1 : -1))
                              .map(([index, instances]) => (
                                <TableRow key={row.id + '-' + index} data-expanded="true">
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
                                                  index,
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
