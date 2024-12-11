import {ReactElement} from 'react';
import {CopyButton} from '@/components/copy-button';
import {cn} from '@/helpers';

type ShortCopyProps = {
  text: string;
  className?: string;
};

const ShortCopy = ({text, className}: ShortCopyProps): ReactElement => {
  return (
    <div className="flex items-center">
      <div
        className={cn(
          'relative me-2 w-40 overflow-hidden whitespace-nowrap font-mono [mask-image:linear-gradient(to_right,white_calc(100%-3rem),transparent_100%)]',
          className,
        )}
        title={text}
      >
        {text}
      </div>
      <CopyButton text={text} variant="outline" size={'icon'} />
    </div>
  );
};

export {ShortCopy};
