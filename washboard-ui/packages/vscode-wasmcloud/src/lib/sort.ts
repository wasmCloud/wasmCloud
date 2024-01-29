export function sortById<T>(items: Record<string, T> | (T & {id: string | number})[]): T[] {
  if (Array.isArray(items)) {
    return items.sort((a, b) => (a < b ? -1 : 1));
  }

  return Object.entries(items)
    .sort(([a], [b]) => (a < b ? -1 : 1))
    .map(([, item]) => item);
}
