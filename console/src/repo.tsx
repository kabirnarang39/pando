import { createContext, useContext, useMemo, type ReactNode } from "react";
import type { Api } from "./api/client";
import { useConnection } from "./connection";

/** The repository a view is about: its name, its key (`OWNER/NAME`) and its API. */
export interface RepoContext {
  owner: string;
  name: string;
  key: string;
  api: Api;
}

const Context = createContext<RepoContext | null>(null);

export function RepoProvider({ owner, name, children }: { owner: string; name: string; children: ReactNode }) {
  const { repoApi } = useConnection();
  const value = useMemo(() => {
    const key = `${owner}/${name}`;
    return { owner, name, key, api: repoApi(key) };
  }, [owner, name, repoApi]);
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export function useRepo(): RepoContext {
  const r = useContext(Context);
  if (!r) throw new Error("useRepo outside a repository");
  return r;
}

/** The repository in view, if any (for components that also work outside one). */
export function useRepoMaybe(): RepoContext | null {
  return useContext(Context);
}

/** Route params for links inside the repository in view. */
export function useRepoParams() {
  const { owner, name } = useRepo();
  return { owner, repo: name };
}
