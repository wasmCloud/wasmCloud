import {zodResolver} from '@hookform/resolvers/zod';
import {ReactElement} from 'react';
import {useFieldArray, useForm, useFormContext} from 'react-hook-form';
import * as z from 'zod';
import {Button} from '@/ui/button';
import {Form, FormControl, FormField, FormItem, FormLabel, FormMessage} from '@/ui/form';
import {Input} from '@/ui/input';
import {useLatticeConfig} from './use-lattice-config';
import {canConnect} from '../services/nats.ts';
import {PlusCircledIcon, MinusCircledIcon} from '@radix-ui/react-icons';
import {SheetFooter} from '@/ui/sheet';
import {credsAuthenticator} from 'nats.ws';

const formSchema = z
  .object({
    authenticators: z.array(
      z.object({
        file: z
          .custom<FileList>()
          .refine((value) => value.length === 1, 'Expected file')
          .transform((value) => value[0] as File),
      }),
    ),
    latticeUrl: z.string().url({message: 'Please enter a valid URL'}),
  })
  .refine(
    async (data) => {
      const tasks = data.authenticators?.filter(isFileAuthenticator).map(toCredsAuthenticator) ?? [];

      return canConnect({
        servers: data.latticeUrl,
        authenticator: await Promise.all(tasks),
      });
    },
    {message: 'Could not connect to Lattice'},
  );

function isFileAuthenticator(value: {file: unknown}) {
  return value.file instanceof File;
}

async function toCredsAuthenticator(value: {file: File}) {
  const arrBuff = await value.file.arrayBuffer();
  return credsAuthenticator(new Uint8Array(arrBuff));
}

function LatticeSettings(): ReactElement {
  const latticeConfig = useLatticeConfig();
  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      latticeUrl: latticeConfig.config.latticeUrl,
      authenticators: undefined,
    },
  });
  // TODO: Handle Root Errors
  //   - Related to: https://github.com/orgs/react-hook-form/discussions/9691#discussioncomment-7655994
  // @ts-expect-error
  const rootError = form.formState.errors[''];
  const submitButtonDisabled = false;
  // const submitButtonDisabled =
  //   Object.keys(form.formState.errors).length > 0 ||
  //   form.formState.isSubmitting ||
  //   !form.formState.isDirty;

  function onSubmit(data: z.infer<typeof formSchema>): void {
    console.log({data});
    // latticeConfig.setConfig('latticeUrl', data.latticeUrl);
  }

  return (
    <div className="flex flex-col gap-3">
      <h3 className="font-semibold">Lattice Configuration</h3>
      <Form {...form}>
        <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-3">
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
                {rootError && (
                  <p className="text-[0.8rem] font-medium text-destructive">{rootError.message}</p>
                )}
              </FormItem>
            )}
          />
          <AuthenticatorsField />
          <SheetFooter className="mt-4">
            <Button variant="default" type="submit" disabled={submitButtonDisabled}>
              Update
            </Button>
          </SheetFooter>
        </form>
      </Form>
    </div>
  );
}

function AuthenticatorsField() {
  const formContext = useFormContext();
  const authenticators = useFieldArray({
    control: formContext.control,
    name: 'authenticators',
  });

  function onAdd() {
    authenticators.append({file: undefined});
  }

  function onRemove(index: number) {
    return () => {
      authenticators.remove(index);
    };
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h4>Authenticators</h4>
        <button onClick={onAdd}>
          <PlusCircledIcon className="w-6 h-6" />
        </button>
      </div>
      <div className="flex flex-col gap-3 divide-gray-500">
        {authenticators.fields.map((field, index) => {
          return (
            <div
              key={field.id}
              className="flex flex-col gap-3 p-2 dark:bg-gray-900 bg-gray-100 rounded"
            >
              <div className="flex items-center gap-2">
                <Input
                  {...formContext.register(`authenticators.${index}.file`)}
                  name={`authenticators.${index}.file`}
                  type="file"
                />
                <button onClick={onRemove(index)}>
                  <MinusCircledIcon className="w-6 h-6 text-red-300" />
                </button>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

export default LatticeSettings;
