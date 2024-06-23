import {zodResolver} from '@hookform/resolvers/zod';
import {canConnect, useLatticeConfig} from '@wasmcloud/lattice-client-react';
import {ReactElement, useEffect} from 'react';
import {useForm} from 'react-hook-form';
import * as z from 'zod';
import {Button} from '@/components/button';
import {Form, FormControl, FormField, FormItem, FormLabel, FormMessage} from '@/components/form';
import {Input} from '@/components/input';
import {SheetClose, SheetFooter} from '@/components/sheet';

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

export function LatticeSettings(): ReactElement {
  const [{latticeUrl, latticeId, ctlTopicPrefix, retryCount}, setConfig] = useLatticeConfig();
  const form = useForm<LatticeFormInput, object, LatticeFormOutput>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      latticeUrl,
      latticeId,
      ctlTopicPrefix,
      retryCount,
    },
  });

  function onSubmit({latticeUrl, latticeId, ctlTopicPrefix, retryCount}: LatticeFormOutput): void {
    setConfig({
      latticeUrl,
      latticeId,
      ctlTopicPrefix,
      retryCount,
    });
  }

  useEffect(() => {
    form.setValue('latticeUrl', latticeUrl);
    form.setValue('latticeId', latticeId);
    form.setValue('ctlTopicPrefix', ctlTopicPrefix);
    form.setValue('retryCount', retryCount);
  }, [form, latticeUrl, ctlTopicPrefix, latticeId, retryCount]);

  const hasErrors = Object.values(form.formState.errors).length > 0;

  return (
    <Form {...form}>
      <form onSubmit={form.handleSubmit(onSubmit)} className="grid gap-4">
        <FormField
          control={form.control}
          name="latticeUrl"
          render={({field}) => (
            <FormItem className="grid w-full max-w-sm items-center gap-1.5">
              <FormLabel htmlFor="latticeUrl">Server URL</FormLabel>
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
            <FormItem className="grid w-full max-w-sm items-center gap-1.5">
              <FormLabel htmlFor="latticeId">Lattice ID</FormLabel>
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
            <FormItem className="grid w-full max-w-sm items-center gap-1.5">
              <FormLabel htmlFor="retryCount">Retry Count</FormLabel>
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
            <FormItem className="grid w-full max-w-sm items-center gap-1.5">
              <FormLabel htmlFor="ctlTopicPrefix">Control Topic Prefix</FormLabel>
              <FormControl>
                <Input type="text" placeholder="wasmbus.ctl" {...field} />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <SheetFooter className="mt-4">
          <SheetClose asChild>
            <Button variant="default" type="submit" disabled={hasErrors}>
              Update
            </Button>
          </SheetClose>
        </SheetFooter>
      </form>
    </Form>
  );
}
