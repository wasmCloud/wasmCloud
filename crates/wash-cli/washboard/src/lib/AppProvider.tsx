import {JSXElementConstructor, ReactElement, ReactNode} from 'react';

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- could be anything for props
interface AppProps<T = any> {
  components: Array<JSXElementConstructor<T>>;
  children: ReactNode;
}

export default function AppProvider(props: AppProps): ReactElement {
  return (
    <>
      {props.components.reduceRight((accumulator, Component) => {
        return <Component>{accumulator}</Component>;
      }, props.children)}
    </>
  );
}
