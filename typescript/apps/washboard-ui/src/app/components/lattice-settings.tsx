import {zodResolver} from '@hookform/resolvers/zod';
import {canConnect, useLatticeConfig} from '@wasmcloud/lattice-client-react';
import {ReactElement, useCallback} from 'react';
import {useForm} from 'react-hook-form';
import * as z from 'zod';
import {Button} from '@/components/button';
import {Form, FormControl, FormField, FormItem, FormLabel, FormMessage} from '@/components/form';
import {Input} from '@/components/input';
import {SheetFooter} from '@/components/sheet';

const formSchema = z.object({
  latticeUrl: z
    .string()
    .url({
      message: 'Please enter a valid URL',
    })
    .refine(
      (latticeId) => {
        return canConnect(latticeId);
      },
      {message: 'Could not connect to Lattice'},
    ),
  latticeId: z.string(),
  ctlTopicPrefix: z.string(),
  retryCount: z.number().or(z.string()).pipe(z.coerce.number().min(0)),
});

type LatticeFormInput = z.input<typeof formSchema>;

type LatticeFormOutput = z.output<typeof formSchema>;

export function LatticeSettings({onSubmit}: {onSubmit?: () => void}): ReactElement {
  const {config, setConfig} = useLatticeConfig();
  const {latticeUrl, latticeId, ctlTopicPrefix, retryCount} = config;

  const form = useForm<LatticeFormInput, object, LatticeFormOutput>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      latticeUrl,
      latticeId,
      ctlTopicPrefix,
      retryCount,
    },
    values: {
      latticeUrl,
      latticeId,
      ctlTopicPrefix,
      retryCount,
    },
  });

  const handleSubmit = useCallback(
    ({latticeUrl, latticeId, ctlTopicPrefix, retryCount}: LatticeFormOutput): void => {
      setConfig({
        latticeUrl,
        latticeId,
        ctlTopicPrefix,
        retryCount,
      });
      onSubmit?.();
    },
    [onSubmit, setConfig],
  );

  return (
    <Form {...form}>
      <form onSubmit={form.handleSubmit(handleSubmit)} className="contents">
        <FormField
          control={form.control}
          name="latticeUrl"
          render={({field}) => (
            <FormItem>
              <FormLabel>Server URL</FormLabel>
              <FormControl>
                <Input type="text" placeholder="ws://server:port" {...field} />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name="latticeId"
          render={({field}) => (
            <FormItem>
              <FormLabel>Lattice ID</FormLabel>
              <FormControl>
                <Input type="text" placeholder="default" {...field} />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name="retryCount"
          render={({field}) => (
            <FormItem>
              <FormLabel>Retry Count</FormLabel>
              <FormControl>
                <Input type="number" min="0" {...field} />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name="ctlTopicPrefix"
          render={({field}) => (
            <FormItem>
              <FormLabel>Control Topic Prefix</FormLabel>
              <FormControl>
                <Input type="text" placeholder="wasmbus.ctl" {...field} />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <SheetFooter className="mt-4">
          <Button variant="default" type="submit">
            Update
          </Button>
        </SheetFooter>
      </form>
    </Form>
  );
}
