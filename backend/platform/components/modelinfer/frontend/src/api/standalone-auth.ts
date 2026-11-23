export async function getVOSBearerToken(): Promise<string | null> {
  return null;
}


export function getSchedulerRuntime(): {
  locale: () => string
  openWeights: () => void
} | undefined {
  return undefined;
}
