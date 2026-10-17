import { createHashHistory, createRootRoute, createRoute, createRouter, lazyRouteComponent } from "@tanstack/react-router";
import { AppShell } from "./shell/AppShell";
import { Empty, ErrorState } from "./ui";
import { RepoLayout } from "./views/repo/RepoLayout";


// Each screen is its own chunk, loaded on first visit (and preloaded on hover).
const HomeView = lazyRouteComponent(() => import("./views/Home"), "HomeView");
const InboxView = lazyRouteComponent(() => import("./views/Inbox"), "InboxView");
const OrgView = lazyRouteComponent(() => import("./views/Org"), "OrgView");
const TokensView = lazyRouteComponent(() => import("./views/Tokens"), "TokensView");
const AgentsView = lazyRouteComponent(() => import("./views/repo/Agents"), "AgentsView");
const ChangesView = lazyRouteComponent(() => import("./views/Changes"), "ChangesView");
const CodeView = lazyRouteComponent(() => import("./views/Code"), "CodeView");
const LocksView = lazyRouteComponent(() => import("./views/Locks"), "LocksView");
const OverviewView = lazyRouteComponent(() => import("./views/repo/Overview"), "OverviewView");
const ReviewView = lazyRouteComponent(() => import("./views/repo/Review"), "ReviewView");
const ReviewsView = lazyRouteComponent(() => import("./views/repo/Reviews"), "ReviewsView");
const RepoSettingsView = lazyRouteComponent(() => import("./views/repo/Settings"), "RepoSettingsView");
const ConnectView = lazyRouteComponent(() => import("./views/Connect"), "ConnectView");

const str = (v: unknown) => (typeof v === "string" && v ? v : typeof v === "number" ? String(v) : undefined);
const flag = (v: unknown) => v === true || v === "true" || v === 1 || undefined;

const root = createRootRoute({ component: AppShell });

const home = createRoute({ getParentRoute: () => root, path: "/", component: HomeView });
const inbox = createRoute({ getParentRoute: () => root, path: "/inbox", component: InboxView });
const connect = createRoute({ getParentRoute: () => root, path: "/connect", component: ConnectView });
const tokens = createRoute({ getParentRoute: () => root, path: "/settings/tokens", component: TokensView });

export type OrgSearch = { tab?: "repositories" | "people" | "agents" | "teams" };
const org = createRoute({
  getParentRoute: () => root,
  path: "/$owner",
  validateSearch: (s): OrgSearch => ({ tab: (["repositories", "people", "agents", "teams"] as const).find((t) => t === s.tab) }),
  component: OrgView,
});

/** `ref` is shared by every view of a repository: the branch being looked at. */
export type RefSearch = { ref?: string };
const repo = createRoute({
  getParentRoute: () => root,
  path: "/$owner/$repo",
  validateSearch: (s): RefSearch => ({ ref: str(s.ref) }),
  component: RepoLayout,
});

const overview = createRoute({ getParentRoute: () => repo, path: "/", component: OverviewView });

export type ChangesSearch = { c?: string };
const changes = createRoute({
  getParentRoute: () => repo,
  path: "/changes",
  validateSearch: (s): ChangesSearch => ({ c: str(s.c) }),
  component: ChangesView,
});

export type CodeSearch = { at?: string; blame?: boolean; history?: boolean; L?: string };
const code = createRoute({
  getParentRoute: () => repo,
  path: "/code/$",
  validateSearch: (s): CodeSearch => ({ at: str(s.at), blame: flag(s.blame), history: flag(s.history), L: str(s.L) }),
  component: CodeView,
});

export type ReviewsSearch = { q?: string; view?: "attention" | "open" | "agents" | "landed" | "closed" };
const reviews = createRoute({
  getParentRoute: () => repo,
  path: "/reviews",
  validateSearch: (s): ReviewsSearch => ({ q: str(s.q), view: (["attention", "open", "agents", "landed", "closed"] as const).find((v) => v === s.view) }),
  component: ReviewsView,
});

export type ReviewSearch = { file?: string; layout?: "unified" | "split" };
const review = createRoute({
  getParentRoute: () => repo,
  path: "/reviews/$number",
  validateSearch: (s): ReviewSearch => ({ file: str(s.file), layout: s.layout === "split" ? "split" : undefined }),
  component: ReviewView,
});

const locks = createRoute({ getParentRoute: () => repo, path: "/locks", component: LocksView });
const agents = createRoute({ getParentRoute: () => repo, path: "/agents", component: AgentsView });
const settings = createRoute({ getParentRoute: () => repo, path: "/settings", component: RepoSettingsView });

// Hash URLs: the app is static files that work from any directory and any host, with deep links.
export const router = createRouter({
  routeTree: root.addChildren([
    home,
    inbox,
    connect,
    tokens,
    org,
    repo.addChildren([overview, changes, code, reviews, review, locks, agents, settings]),
  ]),
  history: createHashHistory(),
  defaultPreload: "intent",
  scrollRestoration: true,
  // A failing view shows what went wrong in place; navigation keeps working.
  defaultErrorComponent: ({ error, reset }) => <ErrorState error={error} retry={reset} title="This view hit an error" />,
  defaultNotFoundComponent: () => <Empty title="There's nothing at this address">Pick a repository on the left, or press / to search.</Empty>,
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
