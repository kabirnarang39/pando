import { createHashHistory, createRootRoute, createRoute, createRouter, redirect } from "@tanstack/react-router";
import { Shell } from "./shell/Shell";
import { ChangesView } from "./views/Changes";
import { CodeView } from "./views/Code";
import { ConnectView } from "./views/Connect";
import { LocksView } from "./views/Locks";
import { Empty, ErrorState } from "./ui";

const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);

/** `ref` is shared by every view: the branch being looked at. */
export type RefSearch = { ref?: string };
const refSearch = (s: Record<string, unknown>): RefSearch => ({ ref: str(s.ref) });

const root = createRootRoute({ component: Shell, validateSearch: refSearch });

const index = createRoute({
  getParentRoute: () => root,
  path: "/",
  beforeLoad: () => {
    throw redirect({ to: "/changes", search: {} });
  },
});

export type ChangesSearch = RefSearch & { c?: string };
const changes = createRoute({
  getParentRoute: () => root,
  path: "/changes",
  validateSearch: (s): ChangesSearch => ({ ...refSearch(s), c: str(s.c) }),
  component: ChangesView,
});

export type CodeSearch = RefSearch & { at?: string; blame?: boolean; history?: boolean; L?: string };
const code = createRoute({
  getParentRoute: () => root,
  path: "/code/$",
  validateSearch: (s): CodeSearch => ({
    ...refSearch(s),
    at: str(s.at),
    blame: s.blame === true || s.blame === "true" || undefined,
    history: s.history === true || s.history === "true" || undefined,
    L: str(s.L) ?? (typeof s.L === "number" ? String(s.L) : undefined),
  }),
  component: CodeView,
});

const locks = createRoute({ getParentRoute: () => root, path: "/locks", validateSearch: refSearch, component: LocksView });
const connect = createRoute({ getParentRoute: () => root, path: "/connect", validateSearch: refSearch, component: ConnectView });

// Hash URLs: the console is static files that work from any directory, any host, with deep links.
export const router = createRouter({
  routeTree: root.addChildren([index, changes, code, locks, connect]),
  history: createHashHistory(),
  defaultPreload: "intent",
  // A failing view shows what went wrong in place; the header, trunk and navigation keep working.
  defaultErrorComponent: ({ error, reset }) => <ErrorState error={error} retry={reset} title="This view hit an error" />,
  defaultNotFoundComponent: () => <Empty title="There's nothing at this address">Use the sections on the left, or press / to search.</Empty>,
  scrollRestoration: true,
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
