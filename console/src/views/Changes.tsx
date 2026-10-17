import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { useEffect, useMemo, useRef, useState } from "react";
import { GridList, GridListItem } from "react-aria-components";
import { title, type Change } from "../api/client";
import { useChange, useHistory, useLive } from "../data";
import { useRefName, typing } from "../shell/hooks";
import { useRepoParams } from "../repo";
import { Author, CopyId, Empty, ErrorState, Icon, Skeleton, Time, useNow } from "../ui";
import { FileDiff, type Layout } from "./DiffView";

/**
 * Everything that landed on a branch, newest first, beside what the selected change did. New
 * landings never move the list under the reader: at the top they slide in; anywhere else
 * they wait behind a "new changes" button.
 */
export function ChangesView() {
  const refName = useRefName();
  const { c: selected } = useSearch({ from: "/$owner/$repo/changes" });
  const navigate = useNavigate({ from: "/$owner/$repo/changes" });
  const live = useLive(refName);
  const [head, setHead] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [initial] = useState(() => new Set<string>());
  const scroller = useRef<HTMLDivElement>(null);
  const pointerInside = useRef(false);

  // Start reading from the tip; changing branch starts over.
  useEffect(() => {
    setHead(null);
  }, [refName]);
  const changes = live.changes;
  useEffect(() => {
    if (head === null && changes?.[0]) {
      setHead(changes[0].id);
      changes.forEach((c) => initial.add(c.id));
    }
  }, [head, changes, initial]);

  const at = changes && head ? changes.findIndex((c) => c.id === head) : -1;
  const waiting = at > 0 ? at : 0;
  const shown = changes ? (at >= 0 ? changes.slice(at) : changes) : [];

  // At the top and not pointing at the list: take new changes in as they land.
  const newest = changes?.[0]?.id;
  useEffect(() => {
    if (newest && waiting > 0 && (scroller.current?.scrollTop ?? 0) < 8 && !pointerInside.current) setHead(newest);
  }, [waiting, newest]);

  const oldest = shown[shown.length - 1];
  const older = useHistory(shown.length >= 100 ? oldest?.parents[0] : undefined);
  const all = useMemo(() => [...shown, ...(older.data?.pages.flat() ?? [])], [shown, older.data]);
  const needle = filter.trim().toLowerCase();
  const visible = needle ? all.filter((c) => c.author.toLowerCase().includes(needle) || c.message.toLowerCase().includes(needle)) : all;

  const select = (id: string | undefined) => navigate({ search: (s) => ({ ...s, c: id }), replace: true });

  // j / k move through the list, as in a mail client.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (typing(e) || e.metaKey || e.ctrlKey || e.altKey || (e.key !== "j" && e.key !== "k")) return;
      const i = visible.findIndex((c) => c.id === selected);
      const next = visible[Math.max(0, Math.min(visible.length - 1, i + (e.key === "j" ? 1 : -1)))];
      if (next) {
        select(next.id);
        document.querySelector(`[data-key="${next.id}"]`)?.scrollIntoView({ block: "nearest" });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const showNew = () => {
    if (newest) setHead(newest);
    scroller.current?.scrollTo({ top: 0, behavior: "smooth" });
  };

  return (
    <div className="split" data-wide data-detail={selected ? "" : undefined}>
      <section className="pane" aria-labelledby="changes-title">
        <div className="pane-head">
          <h1 id="changes-title">Changes</h1>
          <span className="muted mono" style={{ fontSize: "var(--t-xs)" }}>
            {refName}
          </span>
          <input
            className="filter"
            type="search"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter by author or message"
            aria-label="Filter changes by author or message"
            style={{ flex: 1, minWidth: 140, height: 30, marginLeft: "auto" }}
          />
        </div>
        <div
          className="pane-body"
          ref={scroller}
          onPointerEnter={() => (pointerInside.current = true)}
          onPointerLeave={() => (pointerInside.current = false)}
        >
          {waiting > 0 && (
            <div className="pill">
              <button type="button" onClick={showNew}>
                <span className="dot" aria-hidden="true" />
                {waiting >= 100 ? "100+ new changes" : `${waiting} new ${waiting === 1 ? "change" : "changes"}`}
              </button>
            </div>
          )}
          {live.error && !changes ? (
            <ErrorState error={live.error} retry={() => live.tip.refetch()} title="Changes didn't load" />
          ) : live.isPending ? (
            <Skeleton rows={10} />
          ) : live.tip.data === null ? (
            <Empty title={`Nothing has landed on ${refName} yet`}>Land a change with `pando land`, and it shows up here as it happens.</Empty>
          ) : (
            <>
              <GridList
                className="feed"
                aria-label={`Changes on ${refName}, newest first`}
                selectionMode="single"
                selectionBehavior="replace"
                disallowEmptySelection={false}
                items={visible}
                selectedKeys={selected ? [selected] : []}
                onSelectionChange={(keys) => {
                  const k = [...(keys as Set<string>)][0];
                  if (k) select(String(k));
                }}
                renderEmptyState={() => <Empty title="No changes match">Clear the filter to see every change.</Empty>}
              >
                {(c) => <FeedRow change={c} fresh={!initial.has(c.id)} />}
              </GridList>
              {(older.hasNextPage || (shown.length >= 100 && !older.data)) && (
                <button type="button" className="more" onClick={() => older.fetchNextPage()} disabled={older.isFetching}>
                  {older.isFetching ? "Loading older changes…" : "Load older changes"}
                </button>
              )}
            </>
          )}
        </div>
      </section>
      <section className="pane" aria-label="Selected change">
        {selected ? (
          <ChangeDetail key={selected} id={selected} onBack={() => select(undefined)} />
        ) : (
          <Overview changes={changes} onFilter={setFilter} />
        )}
      </section>
    </div>
  );
}

function FeedRow({ change: c, fresh }: { change: Change; fresh: boolean }) {
  const files = c.files.length;
  return (
    <GridListItem id={c.id} textValue={`${title(c.message)}, by ${c.author}`} className="feed-row" data-fresh={fresh || undefined}>
      <div className="feed-title">{title(c.message)}</div>
      <div className="feed-side">
        <Time ms={c.time} />
        <span className="num">
          {files} {files === 1 ? "file" : "files"}
        </span>
      </div>
      <div className="feed-meta">
        <Author author={c.author} />
      </div>
    </GridListItem>
  );
}

const LAYOUT_KEY = "pando.console.diff-layout";
function storedLayout(): Layout {
  try {
    return localStorage.getItem(LAYOUT_KEY) === "split" ? "split" : "unified";
  } catch {
    return "unified";
  }
}

export function ChangeDetail({ id, onBack }: { id: string; onBack(): void }) {
  const rp = useRepoParams();
  const change = useChange(id);
  const [layout, setLayoutState] = useState<Layout>(storedLayout);
  // A file's open state: what the reader chose, else open (only the first 10 in a huge change).
  const [chosen, setChosen] = useState<Map<string, boolean>>(new Map());
  const setLayout = (l: Layout) => {
    setLayoutState(l);
    try {
      localStorage.setItem(LAYOUT_KEY, l);
    } catch {
      // The choice still applies for this visit.
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!typing(e) && !e.metaKey && !e.ctrlKey && e.key === "u") setLayout(layout === "unified" ? "split" : "unified");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (change.isPending) return <Skeleton rows={8} />;
  if (change.error) return <ErrorState error={change.error} retry={() => change.refetch()} title="This change didn't load" />;
  const c = change.data;
  const body = c.message.split("\n").slice(1).join("\n").trim();
  const parent = c.parents[0];
  const many = c.files.length > 25;
  const isOpen = (path: string, i: number) => chosen.get(path) ?? (!many || i < 10);
  const allOpen = c.files.every((f, i) => isOpen(f.path, i));

  return (
    <div className="pane-body" key={id}>
      <header className="change-head">
        <button type="button" className="button back" onClick={onBack} style={{ marginBottom: 12 }}>
          <Icon name="back" /> All changes
        </button>
        <h1>{title(c.message)}</h1>
        {body && <p className="change-body">{body}</p>}
        <dl className="facts">
          <div>
            <dt className="sr-only">Author</dt>
            <dd>
              <Author author={c.author} />
            </dd>
          </div>
          <div>
            <dt>Landed</dt>
            <dd>
              <Time ms={c.time} />
            </dd>
          </div>
          <div>
            <dt>Change</dt>
            <dd>
              <CopyId id={c.id} label="Copy change id" />
            </dd>
          </div>
          {parent && (
            <div>
              <dt>After</dt>
              <dd>
                <Link to="/$owner/$repo/changes" params={rp} search={{ c: parent }} className="mono" style={{ fontSize: "var(--t-xs)" }}>
                  {parent.slice(0, 12)}
                </Link>
              </dd>
            </div>
          )}
          <div>
            <dt>Browse</dt>
            <dd>
              <Link to="/$owner/$repo/code/$" params={{ ...rp, _splat: "" }} search={{ at: c.id }}>
                Files at this change
              </Link>
            </dd>
          </div>
        </dl>
        {c.redacted && <p className="notice" style={{ margin: "12px 0 0" }}>Some files in this change are hidden because your token may not read them.</p>}
      </header>
      <div className="toolbar">
        <span className="num">
          {c.files.length} {c.files.length === 1 ? "file" : "files"} changed
        </span>
        <div className="segmented" role="group" aria-label="Diff layout">
          {(["unified", "split"] as const).map((l) => (
            <button key={l} type="button" aria-pressed={layout === l} onClick={() => setLayout(l)}>
              {l === "unified" ? "Unified" : "Split"}
            </button>
          ))}
        </div>
        {c.files.length > 1 && (
          <button
            type="button"
            className="copy"
            style={{ fontFamily: "var(--sans)" }}
            onClick={() => setChosen(new Map(c.files.map((f) => [f.path, !allOpen])))}
          >
            {allOpen ? "Collapse all" : "Expand all"}
          </button>
        )}
      </div>
      {c.files.map((f, i) => {
        const open = isOpen(f.path, i);
        return (
          <FileDiff
            key={f.path}
            file={f}
            from={parent}
            to={c.id}
            layout={layout}
            open={open}
            onToggle={(o) => o !== open && setChosen((m) => new Map(m).set(f.path, o))}
          />
        );
      })}
    </div>
  );
}

/** The last hour at a glance: who is landing, and where. Shown until a change is picked. */
function Overview({ changes, onFilter }: { changes: Change[] | undefined; onFilter(f: string): void }) {
  const rp = useRepoParams();
  const now = Math.max(useNow(), Date.now());
  if (!changes) return <Skeleton rows={6} />;
  const hour = changes.filter((c) => c.time > now - 3600_000);
  const count = <K extends string>(keys: K[]) => {
    const m = new Map<K, number>();
    keys.forEach((k) => m.set(k, (m.get(k) ?? 0) + 1));
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  };
  const authors = count(hour.map((c) => c.author));
  const areas = count(hour.flatMap((c) => [...new Set(c.files.map((f) => f.path.split("/").slice(0, 2).join("/")))]));
  const max = Math.max(1, authors[0]?.[1] ?? 1, areas[0]?.[1] ?? 1);
  return (
    <div className="pane-body">
      <div className="overview">
        <h2>The last hour</h2>
        <p className="lede">
          {hour.length === 0
            ? "Nothing landed in the last hour."
            : `${hour.length} ${hour.length === 1 ? "change" : "changes"} from ${authors.length} ${authors.length === 1 ? "author" : "authors"}. Pick a change to see every line it touched; j and k move through the list.`}
        </p>
        {hour.length > 0 && (
          <div className="overview-grid">
            <section aria-labelledby="who-title">
              <h3 id="who-title">Who landed</h3>
              <ul className="bars">
                {authors.slice(0, 10).map(([a, n]) => (
                  <li key={a}>
                    <button type="button" onClick={() => onFilter(a)} title={`Show only changes by ${a}`}>
                      <Author author={a} />
                    </button>
                    <span className="bar" style={{ "--w": n / max } as React.CSSProperties} aria-hidden="true" />
                    <span className="num">{n}</span>
                  </li>
                ))}
              </ul>
            </section>
            <section aria-labelledby="where-title">
              <h3 id="where-title">Where</h3>
              <ul className="bars">
                {areas.slice(0, 10).map(([a, n]) => (
                  <li key={a}>
                    <Link to="/$owner/$repo/code/$" params={{ ...rp, _splat: a }} className="mono">
                      {a}
                    </Link>
                    <span className="bar" style={{ "--w": n / max } as React.CSSProperties} aria-hidden="true" />
                    <span className="num">{n}</span>
                  </li>
                ))}
              </ul>
            </section>
          </div>
        )}
      </div>
    </div>
  );
}
