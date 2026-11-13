type Outcome = 'passed' | 'network' | 'auth' | 'unloaded' | 'timeout' | 'failed';
type Reachability = 'reachable' | 'unreachable' | 'unknown';
interface TestResult { outcome: Outcome; reachability: Reachability }
export function lastModelTest(upstream: string, model: string): (TestResult & {at: string}) | undefined;
export function rememberModelTest(upstream: string, model: string, result: TestResult): void;
export function classifyModelTest(status: number, payload: unknown, upstreamResponded: boolean): TestResult;
export const resultLabels: Record<Outcome, string>;
export const reachabilityLabels: Record<Reachability, string>;
export function upstreamHost(address?: string): string;
