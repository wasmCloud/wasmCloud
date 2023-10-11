import {Slot} from '@radix-ui/react-slot';
import {Check, Copy} from 'lucide-react';
import {MouseEvent, ReactNode, forwardRef, useEffect, useState} from 'react';
import {cn} from 'lib/utils';
import {Button} from 'ui/button';
import {ButtonProps} from 'ui/button/Button';

interface CopyButtonProps extends Omit<ButtonProps, 'children'> {
  text: string;
  children?: ReactNode | ((copied: boolean) => ReactNode);
}

const CopyButton = forwardRef<HTMLButtonElement, CopyButtonProps>(
  ({asChild, onClick, children, className, ...props}: CopyButtonProps, ref) => {
    const Comp = asChild ? Slot : Button;
    const [copied, setCopied] = useState(false);

    useEffect(() => {
      if (copied) {
        const timeout = setTimeout(() => setCopied(false), 2000);
        return (): void => clearTimeout(timeout);
      }
    }, [copied]);

    const handleClick = (event: MouseEvent<HTMLButtonElement>): void => {
      navigator.clipboard.writeText(props.text);
      setCopied(true);
      onClick?.(event);
    };

    if (children) {
      return (
        <Comp ref={ref} onClick={handleClick} {...props}>
          {typeof children === 'function' ? children(copied) : children}
        </Comp>
      );
    }

    const iconClass = cn('w-3 h-3');

    return (
      <Comp ref={ref} onClick={handleClick} {...props} className={cn('h-6 w-6', className)}>
        {copied ? <Check className={iconClass} /> : <Copy className={iconClass} />}
      </Comp>
    );
  },
);

CopyButton.displayName = 'CopyButton';

export {CopyButton};
