import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { Button, Tooltip, TooltipTrigger } from "react-aria-components";
import { explain, hex, isNotFound, Mode, type FileChange } from "../../api/client";
import { CheckPill, ReviewButton, reviewState, StatePill, Thread } from "../../components/ReviewParts";
import { ChangeRequestState, Verdict, type Comment } from "../../gen/pando_platform_pb";
import { Markdown } from "../../markdown";
import { useChangeRequest, useCloseChangeRequest, useComments, useLandChangeRequest, useRebaseChangeRequest } from "../../platform-data";
import { useRepo, useRepoParams } from "../../repo";
import { typing } from "../../shell/hooks";
import { Author, Empty, ErrorState, Icon, Skeleton, Time } from "../../ui";
import { FileDiff, type Layout } from "../DiffView";

/**
 * A change request on one page: what it is, whether it can land and why not, the conversation,
 * and every changed file with its comments in place. n / p move between files, a opens review.
 */
export function ReviewView() {
  const { number } = useParams({ from: "/$owner/$repo/reviews/$number" });
  const { key } = useRepo();
  const n = BigInt(/^\d+$/.test(number) ? number : "0");
  const cr = useChangeRequest(key, n);
  if (cr.isPending) return <Skeleton rows={12} />;
  if (cr.error) return isNotFound(cr.error) ? <Empty title={`There's no change request #${number}`}>It may be in another repository.</Empty> : <ErrorState error={cr.error} retry={() => cr.refetch()} />;
  return <Page />;
}

function Page() {
  const { number } = useParams({ from: "/$owner/$repo/reviews/$number" });
  const { layout: layoutParam } = useSearch({ from: "/$owner/$repo/reviews/$number" });
  const navigate = useNavigate({ from: "/$owner/$repo/reviews/$number" });
  const { key, api } = useRepo();
  const rp = useRepoParams();
  const n = BigInt(number);
  const cr = useChangeRequest(key, n).data!;
  const comments = useComments(key, n);
  const land = useLandChangeRequest(key, n);
  const close = useCloseChangeRequest(key, n);
  const rebase = useRebaseChangeRequest(key, n);
  const layout: Layout = layoutParam ?? "unified";
  const base = hex(cr.base);
  const tip = hex(cr.sourceTip);
  const files = useQuery({
    queryKey: [key, "diff", base, tip],
    queryFn: async () => (await api.diff.diff({ from: base, to: tip })).changes.map((c): FileChange => ({ path: c.path, blob: hex(c.blob), mode: c.mode as Mode })),
    staleTime: Infinity,
  });
  const [composing, setComposing] = useState<{ path: string; line: number } | null>(null);

  // Threads by file and line, and the conversation (comments not on a line).
  const { byLine, conversation } = useMemo(() => {
    const byLine = new Map<string, Map<number, Comment[][]>>();
    const threads = new Map<string, Comment[]>();
    for (const c of comments.data ?? []) threads.set(c.thread, [...(threads.get(c.thread) ?? []), c]);
    const conversation: Comment[][] = [];
    for (const t of threads.values()) {
      const h = t[0]!;
      if (!h.path) {
        conversation.push(t);
        continue;
      }
      const m = byLine.get(h.path) ?? new Map<number, Comment[][]>();
      m.set(h.line, [...(m.get(h.line) ?? []), t]);
      byLine.set(h.path, m);
    }
    return { byLine, conversation };
  }, [comments.data]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (typing(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "a") (e.preventDefault(), document.getElementById("review-button")?.click());
      if (e.key === "u") navigate({ search: (s) => ({ ...s, layout: layout === "split" ? undefined : "split" }), replace: true });
      if (e.key === "n" || e.key === "p") {
        const heads = [...document.querySelectorAll<HTMLElement>(".file-diff")];
        const y = document.querySelector(".review-main")!.getBoundingClientRect().top + 60;
        const current = heads.findIndex((h) => h.getBoundingClientRect().top > y);
        const target = e.key === "n" ? heads[current < 0 ? heads.length - 1 : current] : heads[Math.max(0, (current < 0 ? heads.length : current) - 2)];
        target?.scrollIntoView({ block: "start", behavior: "smooth" });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const open = cr.state === ChangeRequestState.OPEN;
  const ready = open && cr.blockers.length === 0;
  const { approved, stale, changes } = reviewState(cr);

  return (
    <div className="review">
      <nav className="review-files" aria-label="Changed files">
        <div className="side-section" style={{ marginTop: 4 }}>
          {files.data ? `${files.data.length} ${files.data.length === 1 ? "file" : "files"} changed` : "Files"}
        </div>
        {files.data?.map((f) => (
          <a key={f.path} href={`#f-${f.path}`} onClick={(e) => (e.preventDefault(), document.getElementById(`f-${f.path}`)?.scrollIntoView({ behavior: "smooth" }))}>
            <span className="path" title={f.path}>
              {f.path}
            </span>
            <span className="muted">{(byLine.get(f.path)?.size ?? 0) > 0 && <Icon name="comment" size={12} label="has comments" />}</span>
          </a>
        ))}
      </nav>
      <div className="review-main">
        <header className="review-head">
          <div className="row-actions">
            <StatePill cr={cr} />
            {cr.fromProtectedLand && (
              <span className="pill" data-tone="info" title="The agent landed on a protected branch, so its change waits here for review">
                <Icon name="shield" size={13} /> From a protected land
              </span>
            )}
          </div>
          <h1>
            {cr.title} <span className="number">#{number}</span>
          </h1>
          <div className="branchline">
            <Author author={cr.author} />
            <span>wants to land</span>
            <span className="branch">{cr.source}</span>
            <span>into</span>
            <span className="branch">{cr.target}</span>
            <span className="muted">
              opened <Time ms={Number(cr.createdAtMs)} />
            </span>
            <span className="muted num">
              <span className="stat-add">+{cr.additions}</span> <span className="stat-del">−{cr.deletions}</span>
            </span>
          </div>
        </header>

        <section className="gate" data-ready={ready || undefined} aria-labelledby="gate-title">
          <span className="gate-icon">
            <Icon name={cr.state === ChangeRequestState.LANDED ? "merge" : ready ? "check" : cr.state === ChangeRequestState.CLOSED ? "x" : "pending"} size={18} />
          </span>
          <div>
            <h2 id="gate-title">
              {cr.state === ChangeRequestState.LANDED
                ? "Landed"
                : cr.state === ChangeRequestState.CLOSED
                  ? "Closed without landing"
                  : ready
                    ? "Ready to land"
                    : "Not ready to land yet"}
            </h2>
            <ul>
              {cr.state === ChangeRequestState.LANDED && (
                <li>
                  As{" "}
                  <Link to="/$owner/$repo/changes" params={rp} search={{ c: hex(cr.landedAs) }} className="mono">
                    {hex(cr.landedAs).slice(0, 12)}
                  </Link>
                </li>
              )}
              {open && cr.blockers.map((b) => <li key={b}>{b}</li>)}
              {open && approved.length > 0 && <li>Approved by {approved.map((r) => r.reviewer).join(", ")}</li>}
              {open && stale.length > 0 && <li>{stale.length} approval on an older version no longer counts</li>}
              {open && cr.conflicts.length > 0 && (
                <li>
                  {cr.target} changed <span className="mono">{cr.conflicts.join(", ")}</span> since this was opened
                </li>
              )}
            </ul>
            {cr.checks.length > 0 && (
              <div className="checks" style={{ marginTop: 8 }}>
                {cr.checks.map((c) => (
                  <CheckPill key={c.name} check={c} />
                ))}
              </div>
            )}
          </div>
          {open && (
            <div className="row-actions">
              {cr.conflicts.length > 0 && (
                <Button className="button" onPress={() => rebase.mutate()} isDisabled={rebase.isPending}>
                  <Icon name="branch" size={15} /> {rebase.isPending ? "Rebasing…" : `Rebase onto ${cr.target}`}
                </Button>
              )}
              <ReviewButton repo={key} cr={cr} />
              <TooltipTrigger delay={200} isDisabled={ready}>
                <Button className="button" data-primary="" isDisabled={!ready || land.isPending} onPress={() => land.mutate()}>
                  <Icon name="merge" size={15} /> {land.isPending ? "Landing…" : "Land"}
                </Button>
                <Tooltip className="tooltip" offset={6}>
                  {cr.blockers.join(". ")}
                </Tooltip>
              </TooltipTrigger>
            </div>
          )}
        </section>
        {(land.error || rebase.error) && (
          <p className="status" data-kind="error" style={{ margin: "12px 28px 0" }} role="alert">
            {explain(land.error ?? rebase.error)}
          </p>
        )}
        {rebase.isSuccess && !rebase.isPending && cr.conflicts.length === 0 && (
          <p className="status" data-kind="ok" style={{ margin: "12px 28px 0" }} role="status">
            Rebased onto {cr.target}. Checks are running again{reviewState(cr).stale.length ? "; earlier approvals need a fresh look" : ""}.
          </p>
        )}

        <section className="conversation" aria-label="Conversation">
          <div className="panel">
            <div className="panel-head">
              <Author author={cr.author} />
              <span className="muted" style={{ fontSize: "var(--t-xs)" }}>
                <Time ms={Number(cr.createdAtMs)} />
              </span>
            </div>
            <div className="panel-body">
              <Markdown text={cr.body || "No description."} />
            </div>
          </div>
          <div className="timeline">
            {cr.reviews.map((r) => (
              <div className="event-line" key={`${r.reviewer}-${String(r.atMs)}`}>
                <Author author={r.reviewer} />
                <span>{r.verdict === Verdict.APPROVED ? "approved" : r.verdict === Verdict.CHANGES_REQUESTED ? "requested changes" : "reviewed"}</span>
                <Time ms={Number(r.atMs)} />
              </div>
            ))}
            {conversation.map((t) => (
              <Thread key={t[0]!.id} repo={key} cr={cr} comments={t} />
            ))}
            {open && <Thread repo={key} cr={cr} comments={[]} />}
          </div>
        </section>

        <div className="toolbar">
          <span className="num">{files.data ? `${files.data.length} files` : "Files"}</span>
          <div className="segmented" role="group" aria-label="Diff layout">
            {(["unified", "split"] as const).map((l) => (
              <button key={l} type="button" aria-pressed={layout === l} onClick={() => navigate({ search: (s) => ({ ...s, layout: l === "split" ? "split" : undefined }), replace: true })}>
                {l === "unified" ? "Unified" : "Split"}
              </button>
            ))}
          </div>
          <span className="muted hide-small">Click a line number to comment. n and p move between files.</span>
          {open && (
            <button type="button" className="button button-quiet" style={{ marginLeft: "auto" }} onClick={() => close.mutate(ChangeRequestState.CLOSED)}>
              Close without landing
            </button>
          )}
        </div>

        {files.isPending && <Skeleton rows={8} />}
        {files.error && <ErrorState error={files.error} />}
        {files.data?.map((f) => {
          const lines = byLine.get(f.path);
          const keep = new Set([...(lines?.keys() ?? []), ...(composing?.path === f.path ? [composing.line] : [])]);
          return (
            <FileDiff
              key={f.path}
              file={f}
              from={base}
              to={tip}
              layout={layout}
              open
              onToggle={() => {}}
              annotations={{
                lines: keep,
                onLine: open ? (line) => setComposing({ path: f.path, line }) : undefined,
                render: (line) => (
                  <>
                    {lines?.get(line)?.map((t) => (
                      <Thread key={t[0]!.id} repo={key} cr={cr} comments={t} />
                    ))}
                    {composing?.path === f.path && composing.line === line && <Thread repo={key} cr={cr} comments={[]} at={composing} onDone={() => setComposing(null)} />}
                  </>
                ),
              }}
            />
          );
        })}
        {changes.length > 0 && <div style={{ height: 24 }} />}
      </div>
    </div>
  );
}
