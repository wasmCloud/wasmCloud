import {ColumnDef, createColumnHelper} from '@tanstack/react-table';
import {useLatticeData, WadmLink} from '@wasmcloud/lattice-client-react';
import * as React from 'react';
import {DataTable} from '@/components/data-table';
import {ShortCopy} from '@/components/short-copy';

const columnHelper = createColumnHelper<WadmLink>();

const columns = [
  columnHelper.display({
    id: 'wit',
    header: 'WIT',
    cell: (info) =>
      `${info.row.original.wit_namespace}:${info.row.original.wit_package}/${info.row.original.interfaces.join(',')}`,
  }),
  columnHelper.accessor('source_id', {
    header: 'Source',
    cell: (info) => <ShortCopy text={info.getValue()} />,
  }),
  columnHelper.accessor('source_config', {
    header: 'Source Config',
  }),
  columnHelper.accessor('target', {
    header: 'Target',
    cell: (info) => <ShortCopy text={info.getValue()} />,
  }),
  columnHelper.accessor('target_config', {
    header: 'Target Config',
  }),
  columnHelper.accessor('name', {
    header: 'Name',
  }),
];

export function LinksTable(): React.ReactElement {
  const {links} = useLatticeData();

  const data = React.useMemo(
    () => Object.values(links).sort((a, b) => (a.source_id > b.source_id ? 1 : -1)),
    [links],
  );

  return (
    <div>
      <DataTable columns={columns as ColumnDef<WadmLink>[]} data={data} />
    </div>
  );
}
