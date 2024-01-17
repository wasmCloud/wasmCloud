import {ReactElement} from 'react';
import {Card} from '@/ui/card';

type StatsTileProps = {
  title: string;
  value: string;
};

export function StatsTile({title, value}: StatsTileProps): ReactElement {
  return (
    <Card variant="accent" className="flex items-center justify-between p-2">
      <div className="me-2 size-2 rounded-full border border-current" role="presentation" />
      <span className="me-auto">{title}</span>
      <span className="ms-2 rounded-full bg-white/50 px-3 py-1 text-sm font-semibold">{value}</span>
    </Card>
  );
}
