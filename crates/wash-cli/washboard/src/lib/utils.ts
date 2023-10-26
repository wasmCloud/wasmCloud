import {type ClassValue, clsx} from 'clsx';
import {twMerge} from 'tailwind-merge';

/**
 * Combine clsx objects into a single string, and merge tailwind classes (i.e. `mb-4 mb-8` becomes `mb-8`)
 * @param inputs 1 or more `ClassValue`s
 * @returns a single CSS class name string, with all clsx objects removed and tailwind classes merged
 * @see {@link https://github.com/lukeed/clsx} for more information on `clsx`
 * @see {@link https://github.com/dcastil/tailwind-merge} for more information on `twMerge`
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
