export function mountGateway(root: ShadowRoot, options: {
  apiBase: string; token: () => Promise<string | null>; clearToken: () => void;
  locale: () => string; visible: () => boolean;
  select: (tab: string) => void;
}): { started: Promise<void>; select: (tab: string) => void; updateLocale: () => void; dispose: () => void }
