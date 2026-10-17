import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import type { DemoPlatform } from "./api/demo-platform";
import { makeApi, repoInterceptor, serverTransport, type Api } from "./api/client";
import type { Transport } from "@connectrpc/connect";

/** Where the app reads from: the built-in demo, or one pandod server (which may host many repositories). */
export type Target = { kind: "demo" } | { kind: "server"; url: string; token: string | null; remember: boolean };

interface Connection {
  target: Target;
  /** Calls that are not about one repository: accounts, organisations, the inbox. */
  api: Api;
  /** Calls about one repository (`OWNER/NAME`). */
  repoApi(repo: string): Api;
  /** A short name for where we are connected, for the header. */
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

/** The saved target. A token is kept for this tab only unless the person chose to remember it. */
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
  // The demo engine is its own chunk: only demo visitors download it.
  const [demo, setDemo] = useState<{ platform: DemoPlatform; transport: typeof import("./api/demo-platform").platformTransport } | null>(null);
  useEffect(() => {
    if (target.kind !== "demo") {
      setDemo(null);
      return;
    }
    let live = true;
    let stop: (() => void) | undefined;
    void import("./api/demo-platform").then(async (m) => {
      const platform = await m.DemoPlatform.create();
      if (!live) return;
      stop = platform.start();
      setDemo({ platform, transport: m.platformTransport });
    });
    return () => {
      live = false;
      stop?.();
    };
  }, [target.kind]);

  const value = useMemo<Connection | null>(() => {
    if (target.kind === "demo" && !demo) return null;
    const transports = new Map<string, Transport>();
    const transport = (repo?: string): Transport => {
      const key = repo ?? "";
      let t = transports.get(key);
      if (!t) {
        t =
          target.kind === "demo"
            ? demo!.transport(demo!.platform, { interceptors: [repoInterceptor(repo)] })
            : serverTransport(target.url, target.token, repo);
        transports.set(key, t);
      }
      return t;
    };
    const apis = new Map<string, Api>();
    const repoApi = (repo: string) => {
      let a = apis.get(repo);
      if (!a) apis.set(repo, (a = makeApi(transport(repo))));
      return a;
    };
    return {
      target,
      api: makeApi(transport()),
      repoApi,
      label: target.kind === "demo" ? "Demo" : hostOf(target.url),
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

  if (!value) return <div className="boot" aria-busy="true" aria-label="Starting the demo" />;
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export function useConnection(): Connection {
  const c = useContext(Context);
  if (!c) throw new Error("useConnection outside ConnectionProvider");
  return c;
}

/** The platform-level API (not about one repository). */
export const usePlatformApi = () => useConnection().api;

function hostOf(url: string) {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}
