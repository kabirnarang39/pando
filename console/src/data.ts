// Every read the console makes. Changesets, files and trees are immutable once addressed by
// id, so they are cached forever; only refs and locks move, and those are polled.

import { keepPreviousData, useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { decodeChange, hex, type Change } from "./api/client";
import { useRepo } from "./repo";

const forever = { staleTime: Infinity, gcTime: 10 * 60_000 } as const;
export const PAGE = 100;

/** The changeset a ref points at, refreshed every two seconds while the tab is visible. */
export function useTip(ref: string) {
  const { api, key: repo } = useRepo();
  return useQuery({
    queryKey: [repo, "tip", ref],
    queryFn: async () => {
      const r = await api.refs.get({ name: ref });
      return r.exists ? hex(r.id) : null;
    },
    refetchInterval: 2000,
  });
}

export function useRefs() {
  const { api, key: repo } = useRepo();
  return useQuery({
    queryKey: [repo, "refs"],
    queryFn: async () => (await api.refs.list({})).refs.map((r) => ({ name: r.name, id: hex(r.id) })),
    refetchInterval: 15_000,
  });
}

export function useWhoAmI() {
  const { api, key: repo } = useRepo();
  return useQuery({ queryKey: [repo, "whoami"], queryFn: () => api.access.whoAmI({}), staleTime: 60_000, retry: false });
}

export function useChange(id: string | undefined) {
  const { api, key: repo } = useRepo();
  return useQuery({
    queryKey: [repo, "change", id],
    queryFn: async () => decodeChange(await api.changesets.get({ rev: id! })),
    enabled: !!id,
    ...forever,
  });
}

async function collect(api: ReturnType<typeof useRepo>["api"], rev: string, path: string, limit: number): Promise<Change[]> {
  const out: Change[] = [];
  for await (const info of api.history.log({ rev, path, limit })) out.push(decodeChange(info));
  return out;
}

/**
 * History from `tip` (an id), newest first, one page at a time. Keyed by the tip id so a page is
 * never refetched; a new tip only fetches what landed since (see `useNewer`).
 */
export function useHistory(tip: string | null | undefined, path = "") {
  const { api, key: repo } = useRepo();
  return useInfiniteQuery({
    queryKey: [repo, "history", tip, path],
    queryFn: ({ pageParam }) => collect(api, pageParam, path, PAGE),
    initialPageParam: tip ?? "",
    getNextPageParam: (last) => (last.length < PAGE ? undefined : last[last.length - 1]?.parents[0]),
    enabled: !!tip,
    ...forever,
  });
}

/** Changes between `known` (exclusive) and `tip`, newest first; at most `PAGE`. */
export function useNewer(tip: string | null | undefined, known: string | null) {
  const { api, key: repo } = useRepo();
  return useQuery({
    queryKey: [repo, "newer", tip, known],
    queryFn: async () => {
      const out = await collect(api, tip!, "", PAGE);
      const at = out.findIndex((c) => c.id === known);
      return at < 0 ? out : out.slice(0, at);
    },
    enabled: !!tip && !!known && tip !== known,
    ...forever,
  });
}

export function useDir(rev: string | undefined, path: string, enabled = true) {
  const { api, key: repo } = useRepo();
  return useQuery({
    queryKey: [repo, "dir", rev, path],
    queryFn: async () => (await api.browse.listDir({ rev: rev!, path })).entries.map((e) => ({ name: e.name, mode: e.mode, id: hex(e.id) })),
    enabled: !!rev && enabled,
    ...forever,
  });
}

export function useFile(rev: string | undefined, path: string, enabled = true) {
  const { api, key: repo } = useRepo();
  return useQuery({
    queryKey: [repo, "file", rev, path],
    queryFn: async () => {
      const r = await api.browse.readFile({ rev: rev!, path });
      return { bytes: r.content, mode: r.mode, id: hex(r.id) };
    },
    enabled: !!rev && !!path && enabled,
    retry: false,
    ...forever,
  });
}

export function useBlame(rev: string | undefined, path: string, enabled: boolean) {
  const { api, key: repo } = useRepo();
  return useQuery({
    queryKey: [repo, "blame", rev, path],
    queryFn: async () => (await api.history.blame({ rev: rev!, path })).lines.map((l) => hex(l.changeset)),
    enabled: !!rev && !!path && enabled,
    ...forever,
  });
}

export function useFind(rev: string | undefined, pattern: string) {
  const { api, key: repo } = useRepo();
  return useQuery({
    queryKey: [repo, "find", rev, pattern],
    queryFn: () => api.browse.find({ rev: rev!, pattern, limit: 50 }),
    enabled: !!rev && pattern.length > 0,
    placeholderData: (prev) => prev,
    ...forever,
  });
}

export function useLocks() {
  const { api, key: repo } = useRepo();
  return useQuery({
    queryKey: [repo, "locks"],
    queryFn: async () => (await api.locks.list({ prefix: "" })).locks.map((l) => ({ path: l.path, owner: l.owner, at: Number(l.lockedAtMs) })),
    refetchInterval: 5000,
  });
}

const LIVE_MAX = 1500;
/** The first load of a live view: enough for an hour of a busy fleet. */
const LIVE_FIRST = 600;

/**
 * The newest changes on `ref`, kept current: each time the tip moves, only the changes that
 * landed since are streamed (the stream stops at the first one already known).
 */
export function useLive(ref: string) {
  const { api, key: repo } = useRepo();
  const qc = useQueryClient();
  const tip = useTip(ref);
  const live = useQuery({
    queryKey: [repo, "live", ref, tip.data],
    queryFn: async () => {
      const prev = qc.getQueryData<Change[]>([repo, "live-latest", ref]) ?? [];
      const known = new Set(prev.slice(0, PAGE).map((c) => c.id));
      const fresh: Change[] = [];
      let joined = prev.length === 0;
      for await (const info of api.history.log({ rev: tip.data!, path: "", limit: prev.length ? PAGE : LIVE_FIRST })) {
        const c = decodeChange(info);
        if (known.has(c.id)) {
          joined = true;
          break;
        }
        fresh.push(c);
      }
      // A gap wider than one page: start over from the new tip rather than show a hole.
      const list = (joined ? [...fresh, ...prev] : fresh).slice(0, LIVE_MAX);
      qc.setQueryData([repo, "live-latest", ref], list);
      return list;
    },
    enabled: !!tip.data,
    placeholderData: keepPreviousData,
    staleTime: Infinity,
    gcTime: 60_000,
  });
  // An empty repository (no tip) has no changes, rather than changes still loading.
  return { tip, changes: tip.data === null ? [] : live.data, error: tip.error ?? live.error, isPending: tip.isPending || (!!tip.data && live.isPending) };
}
