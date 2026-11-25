export function mountGateway(root: ShadowRoot, options: {
  apiBase: string; token: () => Promise<string | null>; clearToken: (rejected?: string) => void;
  locale: () => string; visible: () => boolean; administrator: () => boolean;
  select: (tab: string) => void;
  manageKey?: (upstreamId: string, model: string) => void;
  unavailableServices?: () => Promise<{id:string;name:string}[]>;
  openService?: (id:string) => void;
}): { started: Promise<void>; select: (tab: string) => void; updateLocale: () => void; updatePrivileges: () => void; dispose: () => void }
