import {zodResolver} from '@hookform/resolvers/zod';
import {ReactElement, useEffect} from 'react';
import {useForm} from 'react-hook-form';
import * as z from 'zod';
import {Button} from 'ui/button';
import {Form, FormControl, FormField, FormItem, FormLabel, FormMessage} from 'ui/form';
import {Input} from 'ui/input';
import {useLatticeConfig} from './use-lattice-config';
import {canConnect} from "../services/nats.ts";

type Props = {
  onSave: (event: z.infer<typeof formSchema>) => void;
};

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

function LatticeSettings({onSave}: Props): ReactElement {
  const {
    config: {latticeUrl},
    setConfig,
  } = useLatticeConfig();
  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      latticeUrl: latticeUrl,
    },
  });

  const handleSave = (data: z.infer<typeof formSchema>): void => {
    onSave(data);
    setConfig('latticeUrl', form.getValues('latticeUrl'));
  };

  useEffect(() => {
    form.setValue('latticeUrl', latticeUrl);
  }, [form, latticeUrl]);

  const hasErrors = Object.values(form.formState.errors).length > 0;

  return (
    <Form {...form}>
      <form onSubmit={form.handleSubmit(handleSave)}>
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
        <div className="mt-4 flex justify-end">
          <Button variant="default" type="submit" disabled={hasErrors}>
            Update
          </Button>
        </div>
      </form>
    </Form>
  );
}

export default LatticeSettings;
