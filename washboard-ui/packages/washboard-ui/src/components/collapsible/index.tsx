import * as CollapsiblePrimitive from '@radix-ui/react-collapsible';
import * as React from 'react';

const OpenContext = React.createContext<{
  open: boolean;
  setOpen: React.Dispatch<React.SetStateAction<boolean>>;
}>({
  open: false,
  setOpen: () => null,
});

function CollapsibleController({
  controlledOpen = false,
  children,
}: React.PropsWithChildren<{
  controlledOpen?: boolean;
}>): React.ReactElement {
  const [open, setOpen] = React.useState(controlledOpen);
  return <OpenContext.Provider value={{open, setOpen}}>{children}</OpenContext.Provider>;
}

const Collapsible = React.forwardRef<
  HTMLDivElement,
  CollapsiblePrimitive.CollapsibleProps & React.RefAttributes<HTMLDivElement>
>((props, ref) => {
  const [open, setOpen] = React.useState(false);
  return (
    <OpenContext.Provider value={{open, setOpen}}>
      <CollapsiblePrimitive.Root open={open} onOpenChange={setOpen} ref={ref} {...props} />
    </OpenContext.Provider>
  );
});
Collapsible.displayName = 'Collapsible';

const CollapsibleTrigger = React.forwardRef<
  HTMLButtonElement,
  Omit<CollapsiblePrimitive.CollapsibleTriggerProps, 'children'> &
    React.RefAttributes<HTMLButtonElement> & {
      children: React.ReactElement | ((open: boolean) => React.ReactElement);
    }
>(({children, ...props}, ref) => {
  const {open, setOpen} = React.useContext(OpenContext);
  return (
    <CollapsiblePrimitive.Trigger
      onClick={(): void => setOpen((previous) => !previous)}
      onKeyDown={(event): void => setOpen(event.key === 'Enter' || event.key === ' ')}
      ref={ref}
      tabIndex={0}
      role="button"
      {...props}
    >
      {typeof children === 'function' ? children(open) : children}
    </CollapsiblePrimitive.Trigger>
  );
});
CollapsibleTrigger.displayName = 'CollapsibleTrigger';

const CollapsibleContent = CollapsiblePrimitive.CollapsibleContent;

export {Collapsible, CollapsibleTrigger, CollapsibleContent, CollapsibleController};
