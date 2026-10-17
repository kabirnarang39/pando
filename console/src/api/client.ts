import { fromBinary } from "@bufbuild/protobuf";
import { Code, ConnectError, createClient, type Interceptor, type Transport } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { ChangesetSchema, Mode } from "../gen/pando_objects_pb";
import { Access, Browse, Changesets, Diff, History, Locks, Refs, type ChangesetInfo } from "../gen/pando_pb";
import { Platform, Reviews } from "../gen/pando_platform_pb";

export { Mode };

/** Typed clients for every service the console reads. */
export function makeApi(transport: Transport) {
  return {
    access: createClient(Access, transport),
    browse: createClient(Browse, transport),
    changesets: createClient(Changesets, transport),
    diff: createClient(Diff, transport),
    history: createClient(History, transport),
    locks: createClient(Locks, transport),
    refs: createClient(Refs, transport),
    platform: createClient(Platform, transport),
    reviews: createClient(Reviews, transport),
  };
}
export type Api = ReturnType<typeof makeApi>;

/** The request header naming the repository a call is about (`OWNER/NAME`). */
export const REPO_HEADER = "x-pando-repo";

/** Names `repo` on every call; servers without the header reach their default repository. */
export const repoInterceptor =
  (repo: string | undefined): Interceptor =>
  (next) =>
  (req) => {
    if (repo) req.header.set(REPO_HEADER, repo);
    return next(req);
  };

/** gRPC-Web to a pandod server, with the bearer token on every call. */
export function serverTransport(baseUrl: string, token: string | null, repo?: string): Transport {
  const auth: Interceptor = (next) => (req) => {
    if (token) req.header.set("authorization", `Bearer ${token}`);
    return next(req);
  };
  return createGrpcWebTransport({ baseUrl: baseUrl.replace(/\/+$/, ""), interceptors: [auth, repoInterceptor(repo)] });
}

export function hex(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += b.toString(16).padStart(2, "0");
  return s;
}

/** The first 12 hex digits: enough to be unique in practice, short enough to read. */
export const shortId = (id: string) => id.slice(0, 12);

export interface FileChange {
  path: string;
  /** Blob id after the change; empty when the file was deleted. */
  blob: string;
  mode: Mode;
}

export interface Change {
  id: string;
  parents: string[];
  author: string;
  message: string;
  /** Milliseconds since the epoch, as the author's client recorded it. */
  time: number;
  generation: number;
  files: FileChange[];
  /** Some files were hidden because the caller may not read them. */
  redacted: boolean;
}

export function decodeChange(info: ChangesetInfo): Change {
  const cs = fromBinary(ChangesetSchema, info.encoded);
  return {
    id: hex(info.id),
    parents: cs.parents.map(hex),
    author: cs.author,
    message: cs.message,
    time: Number(cs.timestampMs),
    generation: Number(info.generation),
    files: cs.changes.map((c) => ({ path: c.path, blob: hex(c.blob), mode: c.mode })),
    redacted: info.redacted,
  };
}

/** The first line of a message, which is its title. */
export const title = (message: string) => message.split("\n", 1)[0]?.trim() || "(no message)";

/** What went wrong, and what to do about it, in a sentence a person can act on. */
export function explain(err: unknown, server?: string): string {
  const e = ConnectError.from(err);
  const where = server ? ` ${server}` : " the server";
  switch (e.code) {
    case Code.Unauthenticated:
      return "The server refused your token. Open Connection and enter a valid one.";
    case Code.PermissionDenied:
      return e.rawMessage || "Your token may not read this.";
    case Code.NotFound:
      return e.rawMessage || "Not found at this revision.";
    case Code.FailedPrecondition:
      return /disabled|flag/i.test(e.rawMessage)
        ? `${e.rawMessage}. An administrator can turn it on with PANDO_FLAGS.`
        : e.rawMessage;
    case Code.Unavailable:
    case Code.Unknown:
      if (/fetch|network|load failed/i.test(e.rawMessage) || e.code === Code.Unavailable)
        return `Can't reach${where}. Check the address, and that pandod allows this site with --web-origins ${location.origin}.`;
      return e.rawMessage;
    default:
      return e.rawMessage || e.message;
  }
}

export const isNotFound = (err: unknown) => ConnectError.from(err).code === Code.NotFound;
export const isUnimplemented = (err: unknown) => ConnectError.from(err).code === Code.Unimplemented;
