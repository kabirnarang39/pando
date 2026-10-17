// Reads and writes that are about the platform rather than one repository's files: accounts,
// organisations, repositories, change requests, comments, the inbox and activity.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { create } from "@bufbuild/protobuf";
import { isUnimplemented } from "./api/client";
import { useConnection, usePlatformApi } from "./connection";
import { ChangeRequestState, RepositorySchema, Verdict, type Repository } from "./gen/pando_platform_pb";

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
      return { reviews };
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
  return useQuery({ queryKey: ["repo", key], queryFn: () => or(api.platform.getRepository(ref(key)), singleRepository(label)), refetchInterval: 10_000, retry: false });
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

export function useTokens() {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["tokens"], queryFn: async () => (await or(api.platform.listTokens({}), { tokens: [] })).tokens });
}

export function useGrants(key: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["grants", key], queryFn: async () => (await or(api.platform.listGrants(ref(key)), { grants: [] })).grants });
}

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
    mutationFn: (c: { body: string; path?: string; line?: number; thread?: string; suggestion?: string }) =>
      api.reviews.addComment({ ref: { repository: ref(key), number }, body: c.body, path: c.path ?? "", line: c.line ?? 0, thread: c.thread ?? "", suggestion: c.suggestion ?? "" }),
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
