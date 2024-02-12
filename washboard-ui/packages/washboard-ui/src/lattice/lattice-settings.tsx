import {zodResolver} from '@hookform/resolvers/zod';
import {canConnect, useLatticeConfig} from '@wasmcloud/lattice-client-react';
import {ReactElement, useEffect} from 'react';
import {useForm} from 'react-hook-form';
import * as z from 'zod';
import {Button} from '@/ui/button';
import {Form, FormControl, FormField, FormItem, FormLabel, FormMessage} from '@/ui/form';
import {Input} from '@/ui/input';
import {SheetClose, SheetFooter} from '@/ui/sheet';

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
});

type LatticeFormInput = z.input<typeof formSchema>;

type LatticeFormOutput = z.output<typeof formSchema>;

export function LatticeSettings(): ReactElement {
  const [{latticeUrl}, setConfig] = useLatticeConfig();
  const form = useForm<LatticeFormInput, object, LatticeFormOutput>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      latticeUrl: latticeUrl,
    },
  });

  function onSubmit(data: LatticeFormOutput): void {
    setConfig({
      latticeUrl: data.latticeUrl,
    });
  }

  useEffect(() => {
    form.setValue('latticeUrl', latticeUrl);
  }, [form, latticeUrl]);

  const hasErrors = Object.values(form.formState.errors).length > 0;

  return (
    <Form {...form}>
      <form onSubmit={form.handleSubmit(onSubmit)}>
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
