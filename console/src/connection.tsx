import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { DemoRepo, demoTransport } from "./api/demo";
import { makeApi, serverTransport, type Api } from "./api/client";

/** Where the console reads from: the built-in demo, or one pandod server. */
export type Target = { kind: "demo" } | { kind: "server"; url: string; token: string | null; remember: boolean };

interface Connection {
  target: Target;
  api: Api;
  /** A short name for the repository, shown in the header. */
  label: string;
  connect(target: Target): void;
}

const Context = createContext<Connection | null>(null);
const KEY = "pando.console.connection";

function read<T>(storage: () => Storage, key: string): T | null {
  try {
    const v = storage().getItem(key);
    return v ? (JSON.parse(v) as T) : null;
  } catch {
    return null;
  }
}

function write(storage: () => Storage, key: string, value: unknown) {
  try {
    if (value === null) storage().removeItem(key);
    else storage().setItem(key, JSON.stringify(value));
  } catch {
    // Storage may be unavailable (private mode); the connection then lasts for this tab only.
  }
}

/**
 * The saved target. A token is kept for this tab only unless the person chose to remember it.
 * A `?server=` link (e.g. from `pandod`'s docs) preselects that server.
 */
function initial(): Target {
  const param = new URLSearchParams(location.search).get("server");
  if (param) return { kind: "server", url: param, token: read<string>(() => sessionStorage, KEY + ".token"), remember: false };
  const saved = read<Target>(() => localStorage, KEY);
  if (saved?.kind === "server") {
    const token = saved.remember ? saved.token : read<string>(() => sessionStorage, KEY + ".token");
    return { ...saved, token };
  }
  return { kind: "demo" };
}

export function ConnectionProvider({ children }: { children: ReactNode }) {
  const [target, setTarget] = useState<Target>(initial);
  const demo = useMemo(() => (target.kind === "demo" ? new DemoRepo() : null), [target.kind]);
  useEffect(() => demo?.start(), [demo]);

  const value = useMemo<Connection>(() => {
    const api = makeApi(target.kind === "demo" ? demoTransport(demo!) : serverTransport(target.url, target.token));
    const label = target.kind === "demo" ? "fleet (demo)" : hostOf(target.url);
    return {
      target,
      api,
      label,
      connect(next) {
        if (next.kind === "server") {
          write(() => localStorage, KEY, { ...next, token: next.remember ? next.token : null });
          write(() => sessionStorage, KEY + ".token", next.token);
        } else {
          write(() => localStorage, KEY, null);
        }
        setTarget(next);
      },
    };
  }, [target, demo]);

  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export function useConnection(): Connection {
  const c = useContext(Context);
  if (!c) throw new Error("useConnection outside ConnectionProvider");
  return c;
}

export const useApi = () => useConnection().api;

function hostOf(url: string) {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}
