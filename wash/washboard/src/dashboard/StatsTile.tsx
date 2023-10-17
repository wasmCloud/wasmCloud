import {ReactElement} from 'react';
import {Card} from 'ui/card';

interface StatsTileProps {
  title: string;
  value: string;
}

function StatsTile({title, value}: StatsTileProps): ReactElement {
  return (
    <Card variant="accent" className="flex items-center justify-between p-2">
      <div className="me-2 h-2 w-2 rounded-full border border-current" role="presentation" />
      <span className="me-auto">{title}</span>
      <span className="ms-2 rounded-full bg-white/50 px-3 py-1 text-sm font-semibold">{value}</span>
    </Card>
  );
}

export default StatsTile;
