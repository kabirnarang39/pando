import { NewChangeRequest } from "../../components/NewChangeRequest";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { useEffect, useMemo, useState } from "react";
import { ChecksIcon, ReviewBadge, StateIcon, stateOf } from "../../components/ReviewParts";
import { ChangeRequestState, type ChangeRequest } from "../../gen/pando_platform_pb";
import { useChangeRequests, useViewer } from "../../platform-data";
import { useRepo, useRepoParams } from "../../repo";
import type { ReviewsSearch } from "../../router";
import { typing } from "../../shell/hooks";
import { Author, Empty, ErrorState, Icon, parseAuthor, Skeleton, Time } from "../../ui";

type View = NonNullable<ReviewsSearch["view"]>;

/**
 * Filters a list with the query syntax people already know: words match the title, author or
 * branch; `author:NAME` (with `*`), `is:ready`, `is:blocked`, `is:agent` and `is:person` narrow it.
 */
export function matches(cr: ChangeRequest, query: string): boolean {
  for (const term of query.toLowerCase().split(/\s+/).filter(Boolean)) {
    const [k, v = ""] = term.includes(":") ? (term.split(/:(.*)/) as [string, string]) : ["", term];
    if (k === "author") {
      const re = new RegExp(`^${v.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*")}$`);
      if (!re.test(cr.author.toLowerCase())) return false;
    } else if (k === "is") {
      if (v === "ready" && !(cr.state === ChangeRequestState.OPEN && cr.blockers.length === 0)) return false;
      if (v === "blocked" && !(cr.state === ChangeRequestState.OPEN && cr.blockers.length > 0)) return false;
      if (v === "agent" && !parseAuthor(cr.author).agent) return false;
      if (v === "person" && parseAuthor(cr.author).agent) return false;
      if (["open", "landed", "closed", "draft"].includes(v) && stateOf(cr) !== v) return false;
    } else if (!`${cr.title} ${cr.author} ${cr.source} #${cr.number}`.toLowerCase().includes(v)) return false;
  }
  return true;
}

export function ReviewsView() {
  const { key } = useRepo();
  const rp = useRepoParams();
  const { q = "", view = "attention" } = useSearch({ from: "/$owner/$repo/reviews" });
  const navigate = useNavigate({ from: "/$owner/$repo/reviews" });
  const all = useChangeRequests(key);
  const viewer = useViewer();
  const me = viewer.data?.account?.login ?? "";
  const [query, setQuery] = useState(q);
  const [cursor, setCursor] = useState(0);
  const [creating, setCreating] = useState(false);

  const views: Record<View, { label: string; test(cr: ChangeRequest): boolean }> = {
    attention: {
      label: "Needs you",
      test: (cr) => cr.state === ChangeRequestState.OPEN && (cr.requestedReviewers.includes(me) || (cr.blockers.length === 0 && parseAuthor(cr.author).agent)),
    },
    open: { label: "Open", test: (cr) => cr.state === ChangeRequestState.OPEN },
    agents: { label: "From agents", test: (cr) => cr.state === ChangeRequestState.OPEN && parseAuthor(cr.author).agent },
    landed: { label: "Landed", test: (cr) => cr.state === ChangeRequestState.LANDED },
    closed: { label: "Closed", test: (cr) => cr.state === ChangeRequestState.CLOSED },
  };
  const shown = useMemo(() => (all.data ?? []).filter((cr) => views[view].test(cr) && matches(cr, query)), [all.data, view, query, me]); // eslint-disable-line react-hooks/exhaustive-deps

  // j / k move, Enter opens: a review queue you can clear without the mouse.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (typing(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "j" || e.key === "k") setCursor((c) => Math.max(0, Math.min(shown.length - 1, c + (e.key === "j" ? 1 : -1))));
      if (e.key === "Enter" && shown[cursor]) navigate({ to: "/$owner/$repo/reviews/$number", params: { ...rp, number: String(shown[cursor].number) } });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  useEffect(() => {
    document.querySelector(`[data-cursor="true"]`)?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  return (
    <div className="page">
      <div className="toolbar-row">
        <div className="segmented" role="group" aria-label="Which change requests">
          {(Object.keys(views) as View[]).map((v) => {
            const count = (all.data ?? []).filter(views[v].test).length;
            return (
              <button key={v} type="button" aria-pressed={view === v} onClick={() => (setCursor(0), navigate({ search: (s) => ({ ...s, view: v === "attention" ? undefined : v }), replace: true }))}>
                {views[v].label}
                <span className="count">{count}</span>
              </button>
            );
          })}
        </div>
        <form
          className="query grow"
          role="search"
          onSubmit={(e) => {
            e.preventDefault();
            navigate({ search: (s) => ({ ...s, q: query || undefined }), replace: true });
          }}
        >
          <Icon name="search" size={15} />
          <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="author:agent/*  is:ready  billing" aria-label="Filter change requests" />
        </form>
        {!creating && (
          <button type="button" className="button" data-primary="" onClick={() => setCreating(true)}>
            <Icon name="plus" size={15} /> New change request
          </button>
        )}
      </div>
      {creating && <NewChangeRequest onCancel={() => setCreating(false)} />}
      <div className="panel">
        {all.isPending ? (
          <Skeleton rows={8} />
        ) : all.error ? (
          <ErrorState error={all.error} retry={() => all.refetch()} />
        ) : shown.length === 0 ? (
          <Empty title={view === "attention" ? "Nothing needs you right now" : "No change requests here"}>
            {view === "attention" ? "Reviews requested from you and agents' changes that are ready to land show up here." : query ? "Try a broader filter." : "When an agent lands on a protected branch, its change waits here for review."}
          </Empty>
        ) : (
          <ul className="list" aria-label="Change requests">
            {shown.map((cr, i) => (
              <li key={String(cr.number)}>
                <Link
                  to="/$owner/$repo/reviews/$number"
                  params={{ ...rp, number: String(cr.number) }}
                  className="row"
                  data-selected={i === cursor || undefined}
                  data-cursor={i === cursor}
                  onMouseEnter={() => setCursor(i)}
                >
                  <StateIcon cr={cr} />
                  <div className="row-main">
                    <div className="row-title">{cr.title}</div>
                    <div className="row-meta">
                      <span className="num">#{String(cr.number)}</span>
                      <Author author={cr.author} />
                      <span>
                        opened <Time ms={Number(cr.createdAtMs)} />
                      </span>
                      <span className="mono">{cr.source}</span>
                    </div>
                  </div>
                  <div className="row-side">
                    {cr.state === ChangeRequestState.OPEN &&
                      (cr.blockers.length === 0 ? (
                        <span className="pill" data-tone="warn">
                          <Icon name="merge" size={12} /> Ready to land
                        </span>
                      ) : (
                        <span className="pill hide-small" title={cr.blockers.join(". ")}>
                          {cr.blockers[0]}
                        </span>
                      ))}
                    <ReviewBadge cr={cr} />
                    <ChecksIcon checks={cr.checks} />
                    {cr.comments > 0 && (
                      <span title={`${cr.comments} comments`} style={{ display: "inline-flex", gap: 3, alignItems: "center" }}>
                        <Icon name="comment" size={14} /> {cr.comments}
                      </span>
                    )}
                    <span className="num hide-small" style={{ minWidth: "8ch", textAlign: "right" }}>
                      <span className="stat-add">+{cr.additions}</span> <span className="stat-del">−{cr.deletions}</span>
                    </span>
                  </div>
                </Link>
              </li>
            ))}
          </ul>
        )}
      </div>
      <p className="muted" style={{ fontSize: "var(--t-xs)", marginTop: 10 }}>
        j and k move, Enter opens. Filters: <code>author:agent/*</code> <code>is:ready</code> <code>is:blocked</code> <code>is:person</code>
      </p>
    </div>
  );
}
