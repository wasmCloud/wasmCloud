import {ColumnDef, createColumnHelper} from '@tanstack/react-table';
import {useLatticeData, WadmLink} from '@wasmcloud/lattice-client-react';
import {ReactElement} from 'react';
import {DataTable} from '@/ui/data-table';
import {ShortCopy} from '@/ui/short-copy';

const columnHelper = createColumnHelper<WadmLink>();

const columns = [
  columnHelper.accessor('contract_id', {
    header: 'Contract ID',
  }),
  columnHelper.accessor('link_name', {
    header: 'Link Name',
  }),
  columnHelper.accessor('provider_id', {
    header: 'Provider ID',
    cell: (info) => <ShortCopy text={info.getValue()} />,
  }),
  columnHelper.accessor('actor_id', {
    header: 'Component ID',
    cell: (info) => <ShortCopy text={info.getValue()} />,
  }),
];

export function LinksTable(): ReactElement {
  const {links} = useLatticeData();

  return (
    <div>
      <DataTable data={links} columns={columns as ColumnDef<WadmLink>[]} />
    </div>
  );
}
