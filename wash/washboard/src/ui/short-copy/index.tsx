import {ReactElement} from 'react';
import {cn} from 'lib/utils';
import {CopyButton} from 'ui/copy-button';

interface ShortCopyProps {
  text: string;
  className?: string;
}

const ShortCopy = ({text, className}: ShortCopyProps): ReactElement => {
  return (
    <div className="flex items-center">
      <div
        className={cn(
          'relative me-2 w-20 overflow-hidden font-mono [mask-image:linear-gradient(to_right,white_calc(100%-3rem),transparent_100%)]',
          className,
        )}
      >
        {text}
      </div>
      <CopyButton text={text} variant="outline" size={'icon'} />
    </div>
  );
};

export {ShortCopy};
