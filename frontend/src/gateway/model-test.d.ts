type Outcome = 'passed' | 'network' | 'auth' | 'unloaded' | 'timeout' | 'failed';
type Reachability = 'reachable' | 'unreachable' | 'unknown';
interface TestResult { outcome: Outcome; reachability: Reachability }
export function modelTestScope(apiBase: string, auth?: {auth_method?: string; identity?: {sub?: string}}): string | undefined;
export function lastModelTest(scope: string | undefined, upstream: string, model: string): (TestResult & {at: string; host: string}) | undefined;
export function rememberModelTest(scope: string | undefined, upstream: string, model: string, result: TestResult, address?: string): void;
export function executionLocation(upstream: {id: string; source?: string; base_url?: string} | undefined, vosHost: string): {label: string; host: string};
export function classifyModelTest(status: number, payload: unknown, upstreamResponded: boolean): TestResult;
export const resultLabels: Record<Outcome, string>;
export const reachabilityLabels: Record<Reachability, string>;
export function upstreamHost(address?: string): string;
