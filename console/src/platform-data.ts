// Reads and writes that are about the platform rather than one repository's files: accounts,
// organisations, repositories, change requests, comments, the inbox and activity.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { isUnimplemented } from "./api/client";
import { usePlatformApi } from "./connection";
import { ChangeRequestState, Verdict } from "./gen/pando_platform_pb";

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
        return { platform: true as const, ...(await api.platform.viewer({})) };
      } catch (e) {
        if (isUnimplemented(e)) return { platform: false as const, account: undefined, organizations: [] };
        throw e;
      }
    },
    staleTime: 60_000,
  });
}

export function useRepositories(owner = "") {
  const api = usePlatformApi();
  return useQuery({
    queryKey: ["repos", owner],
    queryFn: async () => (await api.platform.listRepositories({ owner, limit: 200 })).repositories,
    refetchInterval: 10_000,
  });
}

export function useRepository(key: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["repo", key], queryFn: () => api.platform.getRepository(ref(key)), refetchInterval: 10_000, retry: false });
}

export function useOrganization(login: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["org", login], queryFn: () => api.platform.getOrganization({ login }), retry: false });
}

export function useMembers(org: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["members", org], queryFn: async () => (await api.platform.listMembers({ login: org })).members, staleTime: 60_000 });
}

export function useTeams(org: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["teams", org], queryFn: async () => (await api.platform.listTeams({ login: org })).teams, staleTime: 60_000 });
}

export function useTokens() {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["tokens"], queryFn: async () => (await api.platform.listTokens({})).tokens });
}

export function useGrants(key: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["grants", key], queryFn: async () => (await api.platform.listGrants(ref(key))).grants });
}

export function useProtections(key: string) {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["protections", key], queryFn: async () => (await api.reviews.listProtections(ref(key))).protections });
}

export function useChangeRequests(key: string, states: ChangeRequestState[] = []) {
  const api = usePlatformApi();
  return useQuery({
    queryKey: ["crs", key, states],
    queryFn: async () => (await api.reviews.listChangeRequests({ repository: ref(key), states, limit: 200 })).changeRequests,
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
    queryFn: async () => (await api.reviews.listComments({ repository: ref(key), number })).comments,
    refetchInterval: 4000,
  });
}

export function useInbox() {
  const api = usePlatformApi();
  return useQuery({ queryKey: ["inbox"], queryFn: async () => (await api.reviews.inbox({ limit: 100 })).items, refetchInterval: 5000 });
}

export function useEvents(key?: string) {
  const api = usePlatformApi();
  return useQuery({
    queryKey: ["events", key ?? ""],
    queryFn: async () => (await api.reviews.listEvents({ repository: key ? ref(key) : undefined, limit: 40 })).events,
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
