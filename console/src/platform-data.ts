// Reads and writes that are about the platform rather than one repository's files: accounts,
// organisations, repositories, change requests, comments, the inbox and activity.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { create } from "@bufbuild/protobuf";
import { isUnimplemented } from "./api/client";
import { useConnection, usePlatformApi } from "./connection";
import { ChangeRequestState, IssueState, RepositorySchema, Role, Verdict, type Repository, type Visibility } from "./gen/pando_platform_pb";

/**
 * Servers without the platform service (one repository, today's pandod) answer Unimplemented.
 * Every platform read then falls back to what such a server is: one repository, no reviews.
 */
async function or<T, F>(call: Promise<T>, fallback: F): Promise<T | F> {
  try {
    return await call;
  } catch (e) {
    if (isUnimplemented(e)) return fallback;
    throw e;
  }
}

/** The one repository of a server without the platform service. */
export const SINGLE = { owner: "server", name: "repo" };
export const isSingle = (key: string) => key === `${SINGLE.owner}/${SINGLE.name}`;

function singleRepository(label: string): Repository {
  return create(RepositorySchema, { owner: SINGLE.owner, name: SINGLE.name, description: `The repository on ${label}.`, defaultBranch: "main" });
}

const ref = (key: string) => {
  const [owner = "", name = ""] = key.split("/");
  return { owner, name };
};

/**
 * Who is signed in and which organisations they belong to. A server without the platform
 * service (one repository, older version) answers Unimplemented: the app then shows that one
 * repository on its own.
 */
export function useViewer() {
  const api = usePlatformApi();
  return useQuery({
    queryKey: ["viewer"],
    queryFn: async () => {
      try {
        const v = await api.platform.viewer({});
        return { platform: true as const, ...v, principal: v.account?.login ?? "" };
      } catch (e) {
        if (isUnimplemented(e)) {
          const who = await api.access.whoAmI({}).catch(() => null);
          return { platform: false as const, account: undefined, principal: who?.principal ?? "", organizations: [] };
        }
        throw e;
      }
    },
    staleTime: 60_000,
  });
}

/**
 * What the connected server can do. Hosting many repositories and reviewing change requests
 * ship separately; the app shows only what the server answers.
 */
export function useCapabilities() {
  const api = usePlatformApi();
  return useQuery({
    queryKey: ["capabilities"],
    queryFn: async () => {
      let reviews = true;
      try {
        await api.reviews.inbox({ limit: 1 });
      } catch (e) {
        if (isUnimplemented(e)) reviews = false;
        else throw e;
      }
      // Plans (ADR-035): where to upgrade, and whether organisations have limits.
      const plans = await or(api.platform.getCapabilities({}), { billingUrl: "", limits: false, supportUrl: "" });
      return { reviews, billingUrl: plans.billingUrl, limits: plans.limits, supportUrl: plans.supportUrl };
    },
    staleTime: 5 * 60_000,
  });
}

export function useRepositories(owner = "") {
  const api = usePlatformApi();
  const { label } = useConnection();
  return useQuery({
    queryKey: ["repos", owner],
    queryFn: async () => (await or(api.platform.listRepositories({ owner, limit: 200 }), { repositories: [singleRepository(label)] })).repositories,
    refetchInterval: 10_000,
  });
}

export function useRepository(key: string) {
  const api = usePlatformApi();
  const { label } = useConnection();
  const qc = useQueryClient();
  return useQuery({
    queryKey: ["repo", key],
    queryFn: async () => {
      const r = await or(api.platform.getRepository(ref(key)), singleRepository(label));
      // One number everywhere: the sidebar and organisation lists show this repository as just read.
      qc.setQueriesData<Repository[]>({ queryKey: ["repos"] }, (list) => list?.map((x) => (x.owner === r.owner && x.name === r.name ? r : x)));
      return r;
    },
    refetchInterval: 10_000,
    retry: false,
  });
}

export function useOrganization(login: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["org", login], queryFn: () => api.platform.getOrganization({ login }), retry: false });
}

export function useMembers(org: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["members", org], queryFn: async () => (await or(api.platform.listMembers({ login: org }), { members: [] })).members, staleTime: 60_000 });
}

export function useTeams(org: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["teams", org], queryFn: async () => (await or(api.platform.listTeams({ login: org }), { teams: [] })).teams, staleTime: 60_000 });
}

/** An organisation's plan, limits and use this month: for its administrators. */
export function useUsage(org: string, enabled: boolean) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["usage", org], enabled, queryFn: () => api.platform.getUsage({ org, month: "" }), retry: false });
}

export function useTokens() {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["tokens"], queryFn: async () => (await or(api.platform.listTokens({}), { tokens: [] })).tokens });
}

export function useGrants(key: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["grants", key], queryFn: async () => (await or(api.platform.listGrants(ref(key)), { grants: [] })).grants });
}

/** A platform change, then refetching what it affects. */
function usePlatformMutation<V, R>(run: (api: ReturnType<typeof usePlatformApi>, v: V) => Promise<R>, affects: readonly (readonly unknown[])[]) {
  const api = usePlatformApi();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: V) => run(api, v),
    onSuccess: () => Promise.all(affects.map((queryKey) => qc.invalidateQueries({ queryKey }))),
  });
}

export const useCreateToken = () =>
  usePlatformMutation(
    (api, t: { name: string; actsAs: string; repositories: string[]; maxRole: Role; days: number }) =>
      api.platform.createToken({ name: t.name, actsAs: t.actsAs, repositories: t.repositories, maxRole: t.maxRole, expiresAtMs: BigInt(Date.now() + t.days * 86_400_000) }),
    [["tokens"]],
  );

export const useRevokeToken = () => usePlatformMutation((api, id: string) => api.platform.revokeToken({ id }), [["tokens"]]);

export const useSetMember = (org: string) =>
  usePlatformMutation((api, m: { login: string; role: Role }) => api.platform.setMember({ org, login: m.login, role: m.role }), [["members", org], ["org", org], ["viewer"]]);

export function useInvitations(org: string, enabled = true) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["invitations", org], queryFn: async () => (await or(api.platform.listInvitations({ login: org }), { invitations: [] })).invitations, enabled });
}

export const useCreateInvitation = (org: string) =>
  usePlatformMutation((api, i: { email: string; role: Role }) => api.platform.createInvitation({ org, email: i.email, role: i.role }), [["invitations", org]]);

export const useRevokeInvitation = (org: string) => usePlatformMutation((api, id: string) => api.platform.revokeInvitation({ org, id }), [["invitations", org]]);

export const useCreateRepository = () =>
  usePlatformMutation((api, r: { owner: string; name: string; description: string; visibility: Visibility }) => api.platform.createRepository(r), [["repos"], ["viewer"], ["org"]]);

export const useCreateOrganization = () =>
  usePlatformMutation((api, o: { login: string; displayName: string }) => api.platform.createOrganization({ login: o.login, displayName: o.displayName }), [["viewer"], ["repos"]]);

export const useSetGrant = (key: string) =>
  usePlatformMutation((api, g: { principal: string; role: Role }) => api.platform.setGrant({ repository: ref(key), principal: g.principal, role: g.role }), [["grants", key]]);

export const useCreateChangeRequest = (key: string) =>
  usePlatformMutation(
    (api, c: { source: string; target: string; title: string; body: string; draft: boolean; reviewers: string[] }) =>
      api.reviews.createChangeRequest({ repository: ref(key), ...c }),
    [["crs", key], ["repo", key], ["repos"], ["inbox"]],
  );

export function useWebhooks(key: string, enabled: boolean) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["webhooks", key], enabled, queryFn: async () => (await or(api.platform.listWebhooks(ref(key)), { webhooks: [] })).webhooks });
}

export function useDeliveries(id: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["deliveries", id], queryFn: async () => (await api.platform.listWebhookDeliveries({ id })).deliveries, refetchInterval: 5000 });
}

export const useCreateWebhook = (key: string) =>
  usePlatformMutation((api, w: { url: string; events: string[] }) => api.platform.createWebhook({ repository: ref(key), url: w.url, events: w.events }), [["webhooks", key]]);

export const useDeleteWebhook = (key: string) => usePlatformMutation((api, id: string) => api.platform.deleteWebhook({ id }), [["webhooks", key]]);

export const usePingWebhook = (key: string) => usePlatformMutation((api, id: string) => api.platform.pingWebhook({ id }), [["webhooks", key], ["deliveries"]]);

export function useIssues(key: string, states: IssueState[]) {
  const api = usePlatformApi();
  return useQuery({
    queryKey: ["issues", key, states],
    queryFn: async () => (await or(api.issues.listIssues({ repository: ref(key), states }), { issues: [] })).issues,
    refetchInterval: 10_000,
  });
}

export function useIssue(key: string, number: bigint) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["issue", key, String(number)], queryFn: () => api.issues.getIssue({ repository: ref(key), number }), refetchInterval: 5000 });
}

export function useIssueComments(key: string, number: bigint) {
  const api = usePlatformApi();
  return useQuery({
    queryKey: ["issue-comments", key, String(number)],
    queryFn: async () => (await api.issues.listIssueComments({ repository: ref(key), number })).comments,
    refetchInterval: 5000,
  });
}

export const useCreateIssue = (key: string) =>
  usePlatformMutation(
    (api, i: { title: string; body: string; assignees: string[]; labels: string[] }) => api.issues.createIssue({ repository: ref(key), ...i }),
    [["issues", key], ["inbox"]],
  );

export const useUpdateIssue = (key: string, number: bigint) =>
  usePlatformMutation(
    (api, u: { fields: string[]; title?: string; body?: string; state?: IssueState; labels?: string[]; assignees?: string[] }) =>
      api.issues.updateIssue({ ref: { repository: ref(key), number }, ...u }),
    [["issue", key, String(number)], ["issues", key], ["inbox"]],
  );

export const useAddIssueComment = (key: string, number: bigint) =>
  usePlatformMutation((api, body: string) => api.issues.addIssueComment({ ref: { repository: ref(key), number }, body }), [["issue-comments", key, String(number)], ["issue", key, String(number)]]);

export const useSetProtection = (key: string) =>
  usePlatformMutation(
    (api, p: { pattern: string; requiredApprovals: number; requiredChecks: string[]; dismissStale: boolean; agentsLandDirectly: boolean; bypass: string[] }) =>
      api.reviews.setProtection({ repository: ref(key), ...p }),
    [["protections", key], ["crs", key], ["cr", key]],
  );

export const useDeleteProtection = (key: string) =>
  usePlatformMutation((api, pattern: string) => api.reviews.deleteProtection({ repository: ref(key), pattern }), [["protections", key], ["crs", key], ["cr", key]]);

export function useProtections(key: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["protections", key], queryFn: async () => (await or(api.reviews.listProtections(ref(key)), { protections: [] })).protections });
}

export function useChangeRequests(key: string, states: ChangeRequestState[] = []) {
  const api = usePlatformApi();
  return useQuery({
    queryKey: ["crs", key, states],
    queryFn: async () => (await or(api.reviews.listChangeRequests({ repository: ref(key), states, limit: 200 }), { changeRequests: [] })).changeRequests,
    refetchInterval: 4000,
  });
}

export function useChangeRequest(key: string, number: bigint) {
  const api = usePlatformApi();
  return useQuery({
    queryKey: ["cr", key, String(number)],
    queryFn: () => api.reviews.getChangeRequest({ repository: ref(key), number }),
    refetchInterval: 3000,
    retry: false,
  });
}

export function useComments(key: string, number: bigint) {
  const api = usePlatformApi();
  return useQuery({
    queryKey: ["comments", key, String(number)],
    queryFn: async () => (await or(api.reviews.listComments({ repository: ref(key), number }), { comments: [] })).comments,
    refetchInterval: 4000,
  });
}

export function useInbox() {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["inbox"], queryFn: async () => (await or(api.reviews.inbox({ limit: 100 }), { items: [] })).items, refetchInterval: 5000 });
}

export function useEvents(key?: string) {
  const api = usePlatformApi();
  return useQuery({
    queryKey: ["events", key ?? ""],
    queryFn: async () => (await or(api.reviews.listEvents({ repository: key ? ref(key) : undefined, limit: 40 }), { events: [] })).events,
    refetchInterval: 5000,
  });
}

/** After a write, refetch everything that shows change requests. */
function useInvalidateReviews() {
  const qc = useQueryClient();
  return () =>
    Promise.all(["cr", "crs", "comments", "inbox", "events", "repo", "repos"].map((k) => qc.invalidateQueries({ queryKey: [k] })));
}

export function useSubmitReview(key: string, number: bigint) {
  const api = usePlatformApi();
  const invalidate = useInvalidateReviews();
  return useMutation({
    mutationFn: (v: { verdict: Verdict; body: string }) => api.reviews.submitReview({ ref: { repository: ref(key), number }, ...v }),
    onSuccess: invalidate,
  });
}

export function useAddComment(key: string, number: bigint) {
  const api = usePlatformApi();
  const invalidate = useInvalidateReviews();
  return useMutation({
    mutationFn: (c: { body: string; path?: string; line?: number; endLine?: number; thread?: string; suggestion?: string }) =>
      api.reviews.addComment({
        ref: { repository: ref(key), number },
        body: c.body,
        path: c.path ?? "",
        line: c.line ?? 0,
        endLine: c.endLine ?? 0,
        thread: c.thread ?? "",
        suggestion: c.suggestion ?? "",
      }),
    onSuccess: invalidate,
  });
}

export function useResolveThread(key: string, number: bigint) {
  const api = usePlatformApi();
  const invalidate = useInvalidateReviews();
  return useMutation({
    mutationFn: (v: { thread: string; resolved: boolean }) => api.reviews.resolveThread({ ref: { repository: ref(key), number }, ...v }),
    onSuccess: invalidate,
  });
}

export function useLandChangeRequest(key: string, number: bigint) {
  const api = usePlatformApi();
  const invalidate = useInvalidateReviews();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api.reviews.land({ repository: ref(key), number }),
    onSuccess: async () => {
      await invalidate();
      await qc.invalidateQueries({ queryKey: [key] });
    },
  });
}

export function useRebaseChangeRequest(key: string, number: bigint) {
  const api = usePlatformApi();
  const invalidate = useInvalidateReviews();
  return useMutation({ mutationFn: () => api.reviews.rebase({ repository: ref(key), number }), onSuccess: invalidate });
}

export function useCloseChangeRequest(key: string, number: bigint) {
  const api = usePlatformApi();
  const invalidate = useInvalidateReviews();
  return useMutation({
    mutationFn: (state: ChangeRequestState) => api.reviews.updateChangeRequest({ ref: { repository: ref(key), number }, fields: ["state"], state }),
    onSuccess: invalidate,
  });
}
