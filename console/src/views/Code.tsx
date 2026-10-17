import { useQueries } from "@tanstack/react-query";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { Fragment, useEffect, useMemo, useState } from "react";
import { Collection, Tree, TreeItem, TreeItemContent } from "react-aria-components";
import { decodeChange, hex, isNotFound, Mode, title, type Change } from "../api/client";
import { useRepo, useRepoParams } from "../repo";
import { useBlame, useDir, useFile, useHistory, useLocks, useTip } from "../data";
import { HIGHLIGHT_LIMIT, useHighlight, type Token } from "../highlight";
import { typing, useRefName } from "../shell/hooks";
import { ago, asText, Author, CopyId, Empty, ErrorState, formatBytes, Icon, Skeleton, Time, useNow } from "../ui";

const parentOf = (p: string) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/")) : "");
const ancestors = (p: string) => p.split("/").slice(0, -1).map((_, i, a) => a.slice(0, i + 1).join("/"));

export function CodeView() {
  const rp = useRepoParams();
  const refName = useRefName();
  const { _splat } = useParams({ from: "/$owner/$repo/code/$" });
  const path = (_splat ?? "").replace(/^\/+|\/+$/g, "");
  const { at } = useSearch({ from: "/$owner/$repo/code/$" });
  const tip = useTip(refName);
  const rev = at ?? tip.data ?? undefined;
  const parent = useDir(rev, parentOf(path), !!path);
  const entry = path ? parent.data?.find((e) => e.name === path.split("/").pop()) : undefined;
  const isDir = !path || entry?.mode === Mode.DIR;

  let body;
  if (tip.error && !rev) body = <ErrorState error={tip.error} retry={() => tip.refetch()} />;
  else if (tip.data === null && !at) body = <Empty title={`${refName} has no files yet`}>Land a change and its files appear here.</Empty>;
  else if (!rev || (path && parent.isPending)) body = <Skeleton rows={12} />;
  else if (path && parent.data && !entry) body = <Empty title="Not at this revision">{path} doesn't exist on {at ? "this change" : refName}. It may have been moved or deleted.</Empty>;
  else if (isDir) body = <DirView rev={rev} path={path} />;
  else body = <FileView rev={rev} path={path} pinned={!!at} />;

  return (
    <div className="split" data-detail={path ? "" : undefined}>
      <nav className="pane" aria-label="Files">
        <div className="pane-head">
          <h1>Files</h1>
          <button
            type="button"
            className="copy"
            style={{ fontFamily: "var(--sans)" }}
            onClick={() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "t" }))}
            title="Go to file (t)"
          >
            <Icon name="search" size={12} /> Go to file <kbd>t</kbd>
          </button>
          {at ? (
            <span className="muted" style={{ fontSize: "var(--t-xs)" }}>
              at <span className="mono">{at.slice(0, 12)}</span>{" "}
              <Link to="/$owner/$repo/code/$" params={{ owner: rp.owner, repo: rp.repo, _splat: path }} search={{}}>
                latest
              </Link>
            </span>
          ) : (
            <span className="muted mono" style={{ fontSize: "var(--t-xs)" }}>
              {refName}
            </span>
          )}
        </div>
        <div className="pane-body">{rev ? <FileTree rev={rev} path={path} /> : <Skeleton rows={8} />}</div>
      </nav>
      <section className="pane" aria-label={path || "Repository root"}>
        {body}
      </section>
    </div>
  );
}

interface Node {
  id: string;
  name: string;
  dir: boolean;
  children?: Node[];
}

/** The repository as a tree; directories load the first time they open. */
function FileTree({ rev, path }: { rev: string; path: string }) {
  const rp = useRepoParams();
  const { api, key: repoKey } = useRepo();
  const navigate = useNavigate();
  const locks = useLocks();
  const locked = new Set(locks.data?.map((l) => l.path));
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(ancestors(path)));
  useEffect(() => {
    setExpanded((s) => (ancestors(path).every((a) => s.has(a)) ? s : new Set([...s, ...ancestors(path)])));
  }, [path]);

  const dirs = ["", ...[...expanded]];
  const results = useQueries({
    queries: dirs.map((d) => ({
      queryKey: [repoKey, "dir", rev, d],
      queryFn: async () => (await api.browse.listDir({ rev, path: d })).entries.map((e) => ({ name: e.name, mode: e.mode, id: hex(e.id) })),
      staleTime: Infinity,
    })),
  });
  const listing = new Map(dirs.map((d, i) => [d, results[i]?.data]));

  const build = (dir: string): Node[] | undefined =>
    listing
      .get(dir)
      ?.slice()
      .sort((a, b) => Number(b.mode === Mode.DIR) - Number(a.mode === Mode.DIR) || a.name.localeCompare(b.name))
      .map((e) => {
        const id = dir ? `${dir}/${e.name}` : e.name;
        const isDir = e.mode === Mode.DIR;
        return { id, name: e.name, dir: isDir, children: isDir && expanded.has(id) ? (build(id) ?? [{ id: `${id}/…`, name: "Loading…", dir: false }]) : isDir ? [] : undefined };
      });
  const items = build("");
  if (results[0]?.error) return <ErrorState error={results[0].error} />;
  if (!items) return <Skeleton rows={10} />;

  const render = (n: Node) => (
    <TreeItem
      id={n.id}
      textValue={n.name}
      className="tree-item"
      hasChildItems={n.dir}
      data-dir={n.dir || undefined}
      onAction={() => {
        if (n.id.endsWith("/…")) return;
        navigate({ to: "/$owner/$repo/code/$", params: { owner: rp.owner, repo: rp.repo, _splat: n.id }, search: (s) => ({ at: (s as { at?: string }).at }) });
        if (n.dir) setExpanded((s) => new Set(s).add(n.id));
      }}
    >
      <TreeItemContent>
        {({ hasChildItems }) => (
          <>
            {hasChildItems ? (
              <span className="chevron" aria-hidden="true">
                <Icon name="chevron" size={12} />
              </span>
            ) : (
              <span style={{ width: 16, flex: "none" }} />
            )}
            <span className="name">{n.name}</span>
            {locked.has(n.id) && (
              <span className="tree-lock" title="Locked">
                <Icon name="lock" size={12} label="locked" />
              </span>
            )}
          </>
        )}
      </TreeItemContent>
      {n.children && <Collection items={n.children}>{render}</Collection>}
    </TreeItem>
  );

  return (
    <Tree
      className="tree"
      aria-label="Files"
      items={items}
      selectionMode="single"
      selectedKeys={path ? [path] : []}
      expandedKeys={expanded}
      onExpandedChange={(keys) => setExpanded(new Set([...keys].map(String)))}
    >
      {render}
    </Tree>
  );
}

function Crumbs({ path }: { path: string }) {
  const rp = useRepoParams();
  const parts = path ? path.split("/") : [];
  return (
    <nav className="crumbs" aria-label="Path">
      <Link to="/$owner/$repo/code/$" params={{ owner: rp.owner, repo: rp.repo, _splat: "" }} search={(s) => ({ at: (s as { at?: string }).at })}>
        root
      </Link>
      {parts.map((p, i) => {
        const to = parts.slice(0, i + 1).join("/");
        const last = i === parts.length - 1;
        return (
          <Fragment key={to}>
            <span className="sep">/</span>
            {last ? (
              <span aria-current="page">{p}</span>
            ) : (
              <Link to="/$owner/$repo/code/$" params={{ owner: rp.owner, repo: rp.repo, _splat: to }} search={(s) => ({ at: (s as { at?: string }).at })}>
                {p}
              </Link>
            )}
          </Fragment>
        );
      })}
    </nav>
  );
}

function DirView({ rev, path }: { rev: string; path: string }) {
  const rp = useRepoParams();
  const dir = useDir(rev, path);
  const history = useHistory(rev, path);
  const last = history.data?.pages[0]?.[0];
  const readme = dir.data?.find((e) => /^readme(\.md|\.txt)?$/i.test(e.name));
  return (
    <>
      <div className="pane-head">
        <Crumbs path={path} />
        {last && (
          <span className="file-meta">
            <Author author={last.author} compact />
            <Link to="/$owner/$repo/changes" params={rp} search={{ c: last.id }} style={{ maxWidth: "36ch", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {title(last.message)}
            </Link>
            <Time ms={last.time} />
          </span>
        )}
      </div>
      <div className="pane-body">
        {dir.isPending ? (
          <Skeleton />
        ) : dir.error ? (
          <ErrorState error={dir.error} retry={() => dir.refetch()} />
        ) : (
          <>
            <table className="table">
              <caption className="sr-only">Contents of {path || "the repository root"}</caption>
              <tbody>
                {dir.data
                  .slice()
                  .sort((a, b) => Number(b.mode === Mode.DIR) - Number(a.mode === Mode.DIR) || a.name.localeCompare(b.name))
                  .map((e) => (
                    <tr key={e.name}>
                      <td style={{ width: 24, color: "var(--fg-3)" }}>
                        <Icon name={e.mode === Mode.DIR ? "chevron" : "file"} size={14} />
                      </td>
                      <td>
                        <Link to="/$owner/$repo/code/$" params={{ ...rp, _splat: path ? `${path}/${e.name}` : e.name }} search={(s) => ({ at: (s as { at?: string }).at })} className={e.mode === Mode.DIR ? undefined : "mono"}>
                          {e.name}
                          {e.mode === Mode.DIR && "/"}
                        </Link>
                      </td>
                    </tr>
                  ))}
              </tbody>
            </table>
            {readme && <Readme rev={rev} path={path ? `${path}/${readme.name}` : readme.name} />}
          </>
        )}
      </div>
    </>
  );
}

function Readme({ rev, path }: { rev: string; path: string }) {
  const file = useFile(rev, path);
  const text = file.data ? asText(file.data.bytes) : null;
  if (!text) return null;
  return (
    <section aria-label={path} style={{ margin: 24, border: "1px solid var(--line)", borderRadius: "var(--r-md)", background: "var(--surface)" }}>
      <div className="pane-head" style={{ borderRadius: "var(--r-md) var(--r-md) 0 0" }}>
        <span className="mono" style={{ fontSize: "var(--t-sm)" }}>
          {path.split("/").pop()}
        </span>
      </div>
      <pre style={{ margin: 0, padding: "16px 20px", maxWidth: "88ch", font: "400 var(--t-md)/1.6 var(--sans)", whiteSpace: "pre-wrap" }}>{text}</pre>
    </section>
  );
}

const CHUNK = 200;

function parseLines(L: string | undefined): [number, number] | null {
  const m = L?.match(/^(\d+)(?:-(\d+))?$/);
  if (!m) return null;
  const a = Number(m[1]);
  const b = Number(m[2] ?? m[1]);
  return [Math.min(a, b), Math.max(a, b)];
}

function FileView({ rev, path, pinned }: { rev: string; path: string; pinned: boolean }) {
  const rp = useRepoParams();
  const { blame, history, L } = useSearch({ from: "/$owner/$repo/code/$" });
  const navigate = useNavigate({ from: "/$owner/$repo/code/$" });
  const file = useFile(rev, path);
  const locks = useLocks();
  const lock = locks.data?.find((l) => l.path === path);
  const text = useMemo(() => (file.data ? asText(file.data.bytes) : null), [file.data]);
  const lines = useMemo(() => (text === null ? [] : text.replace(/\n$/, "").split("\n")), [text]);
  const tokens = useHighlight(file.data?.id, path, text);
  const picked = parseLines(L);

  const toggle = (key: "blame" | "history") => navigate({ search: (s) => ({ ...s, [key]: s[key] ? undefined : true }), replace: true });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (typing(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "b") toggle("blame");
      if (e.key === "h") toggle("history");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  useEffect(() => {
    if (picked) document.getElementById(`L${picked[0]}`)?.scrollIntoView({ block: "center" });
    // Only when the file first renders, not on every selection.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [file.data?.id]);

  const pick = (n: number, extend: boolean) =>
    navigate({ search: (s) => ({ ...s, L: extend && picked ? `${Math.min(picked[0], n)}-${Math.max(picked[0], n)}` : String(n) }), replace: true });

  if (file.isPending) return <Skeleton rows={14} />;
  if (file.error) return isNotFound(file.error) ? <Empty title="Not at this revision">{path} doesn't exist here.</Empty> : <ErrorState error={file.error} retry={() => file.refetch()} />;

  const size = file.data.bytes.length;
  return (
    <>
      <div className="pane-head">
        <Link to="/$owner/$repo/code/$" params={{ owner: rp.owner, repo: rp.repo, _splat: parentOf(path) }} search={(s) => ({ at: (s as { at?: string }).at })} className="button back" aria-label="Back to folder">
          <Icon name="back" />
        </Link>
        <Crumbs path={path} />
        <div className="file-meta">
          {lock && (
            <span style={{ color: "var(--gold-ink)" }} title={`Locked by ${lock.owner}`}>
              <Icon name="lock" size={12} /> Locked by {lock.owner}
            </span>
          )}
          {text !== null && (
            <span className="num">
              {lines.length.toLocaleString()} {lines.length === 1 ? "line" : "lines"}
            </span>
          )}
          <span className="num">{formatBytes(size)}</span>
          {pinned && <span>at a past change</span>}
          {text !== null && (
            <div className="segmented" role="group" aria-label="File view">
              <button type="button" aria-pressed={!!blame} onClick={() => toggle("blame")} title="Blame (b)">
                Blame
              </button>
              <button type="button" aria-pressed={!!history} onClick={() => toggle("history")} title="History (h)">
                History
              </button>
            </div>
          )}
        </div>
      </div>
      <div className="pane-body">
        {history ? (
          <FileHistory rev={rev} path={path} />
        ) : text === null ? (
          <p className="notice">Binary file, {formatBytes(size)}. It can't be shown here; check it out with pando clone or mount.</p>
        ) : (
          <>
            {text.length > HIGHLIGHT_LIMIT && <p className="notice">Highlighting is off for files over 1 MB.</p>}
            <Code lines={lines} tokens={tokens} picked={picked} onPick={pick} blame={blame ? { rev, path } : null} />
          </>
        )}
      </div>
    </>
  );
}

function Code({
  lines,
  tokens,
  picked,
  onPick,
  blame,
}: {
  lines: string[];
  tokens: Token[][] | null;
  picked: [number, number] | null;
  onPick(n: number, extend: boolean): void;
  blame: { rev: string; path: string } | null;
}) {
  const rp = useRepoParams();
  const owners = useBlame(blame?.rev, blame?.path ?? "", !!blame);
  const ids = useMemo(() => [...new Set(owners.data ?? [])], [owners.data]);
  const { api, key: repoKey } = useRepo();
  const changes = useQueries({
    queries: ids.map((id) => ({
      queryKey: [repoKey, "change", id],
      queryFn: async () => decodeChange(await api.changesets.get({ rev: id })),
      staleTime: Infinity,
    })),
  });
  const byId = new Map<string, Change>();
  changes.forEach((q) => q.data && byId.set(q.data.id, q.data));
  const now = useNow();

  const chunks = [];
  for (let start = 0; start < lines.length; start += CHUNK) {
    const end = Math.min(lines.length, start + CHUNK);
    chunks.push(
      <div className="code-chunk" key={start} style={{ "--rows": end - start } as React.CSSProperties}>
        {lines.slice(start, end).map((line, k) => {
          const i = start + k;
          const n = i + 1;
          const owner = owners.data?.[i];
          const first = !!owner && owners.data?.[i - 1] !== owner;
          const c = owner ? byId.get(owner) : undefined;
          return (
            <div className="line" id={`L${n}`} key={n} data-picked={picked && n >= picked[0] && n <= picked[1] ? "" : undefined}>
              {blame && (
                <span
                  className="blame-cell"
                  data-first={first || undefined}
                  title={c ? `${title(c.message)}, ${c.author}, ${ago(c.time, now)}` : undefined}
                  style={{ "--heat": c ? Math.max(0, 1 - (now - c.time) / (6 * 3600_000)) : 0 } as React.CSSProperties}
                >
                  {first && c && (
                    <>
                      <Author author={c.author} compact />
                      <Link to="/$owner/$repo/changes" params={rp} search={{ c: c.id }} title={c.message}>
                        {title(c.message)}
                      </Link>
                      <span className="age num">{ago(c.time, now)}</span>
                    </>
                  )}
                </span>
              )}
              <span className="ln num" onClick={(e) => onPick(n, e.shiftKey)} aria-hidden="true">
                {n}
              </span>
              <span className="src">{tokens?.[i] ? tokens[i]!.map(([t, color], j) => (color ? <span key={j} style={{ color }}>{t}</span> : t)) : line || " "}</span>
            </div>
          );
        })}
      </div>,
    );
  }
  return (
    <div className="code" style={{ "--gutter": blame ? "300px" : "0px" } as React.CSSProperties} aria-label="File contents">
      {blame && owners.isPending && <p className="notice">Loading blame…</p>}
      {blame && owners.error && <ErrorState error={owners.error} title="Blame didn't load" />}
      {chunks}
    </div>
  );
}

function FileHistory({ rev, path }: { rev: string; path: string }) {
  const rp = useRepoParams();
  const history = useHistory(rev, path);
  if (history.isPending) return <Skeleton />;
  if (history.error) return <ErrorState error={history.error} retry={() => history.refetch()} />;
  const all = history.data.pages.flat();
  return (
    <>
      <table className="table">
        <caption className="sr-only">Changes to {path}, newest first</caption>
        <tbody>
          {all.map((c) => (
            <tr key={c.id}>
              <td>
                <Link to="/$owner/$repo/changes" params={rp} search={{ c: c.id }}>
                  {title(c.message)}
                </Link>
              </td>
              <td>
                <Author author={c.author} />
              </td>
              <td className="muted" style={{ whiteSpace: "nowrap" }}>
                <Time ms={c.time} />
              </td>
              <td>
                <Link to="/$owner/$repo/code/$" params={{ owner: rp.owner, repo: rp.repo, _splat: path }} search={{ at: c.id }}>
                  View file
                </Link>{" "}
                <CopyId id={c.id} label="Copy change id" />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {history.hasNextPage && (
        <button type="button" className="more" onClick={() => history.fetchNextPage()} disabled={history.isFetchingNextPage}>
          {history.isFetchingNextPage ? "Loading…" : "Load older changes"}
        </button>
      )}
    </>
  );
}
