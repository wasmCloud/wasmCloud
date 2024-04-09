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
import {WadmConfig, useLatticeData} from '@wasmcloud/lattice-client-react';
import {ChevronDown, ChevronRight} from 'lucide-react';
import {Fragment, ReactElement, useState, useMemo} from 'react';
import {Collapsible, CollapsibleContent, CollapsibleTrigger} from '@/components/collapsible';
import {Table, TableHeader, TableRow, TableHead, TableBody, TableCell} from '@/components/table';

const columnHelper = createColumnHelper<WadmConfig>();

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
    meta: {
      baseRow: 'visible',
      expandedRow: 'empty',
    },
  }),
  columnHelper.accessor('entries', {
    header: 'Key',
    id: 'key',
    cell: (info) => {
      const count = Object.keys(info.getValue()).length;
      return count === 1 ? '1 entry' : `${count} entries`;
    },
    meta: {
      baseRow: 'visible',
      expandedRow: 'visible',
      expandedCell: (key) => () => <span className="ms-4">{key}</span>,
    },
  }),
  columnHelper.accessor('entries', {
    header: 'Value',
    id: 'value',
    meta: {
      baseRow: 'empty',
      expandedRow: 'visible',
      expandedCell: (_key, value) => () => value,
    },
  }),
];

export function ConfigsTable(): ReactElement {
  const {configs} = useLatticeData();

  const data = useMemo(
    () => Object.values(configs).sort((a, b) => (a.name > b.name ? 1 : -1)),
    [configs],
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
                          {Object.keys(row.getValue('key')).length > 0 &&
                            Object.entries(row.getValue('key') as WadmConfig['entries'])
                              .sort((a, b) => (a[0] > b[0] ? 1 : -1))
                              .map(([key, entries]) => (
                                <TableRow key={row.id + '-' + key} data-expanded="true">
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
                                                  key,
                                                  entries,
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
