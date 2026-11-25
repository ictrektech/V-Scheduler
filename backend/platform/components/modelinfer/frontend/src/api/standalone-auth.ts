export async function getVOSBearerToken(): Promise<string | null> {
  return null;
}


export function getSchedulerRuntime(): {
  locale: () => string
  openWeights: () => void
} | undefined {
  return undefined;
}

export function isRuntimeAuthError(_error: unknown): boolean { return false; }

export async function withRuntimeToken<T>(_send: (token: string) => Promise<T>, _read: boolean): Promise<T> {
  throw new Error('V-Scheduler authentication is unavailable in standalone mode');
}
