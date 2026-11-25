export function getSchedulerRuntime(): { locale: () => string } | undefined {
  return undefined;
}

export async function withRuntimeToken<T>(
  _send: (token: string) => Promise<T>,
  _read: boolean,
): Promise<T> {
  throw new Error('V-Scheduler authentication is unavailable in standalone mode');
}
