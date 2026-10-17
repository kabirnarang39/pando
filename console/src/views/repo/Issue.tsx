import { Link, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { IssueStateIcon, issueWord } from "../../components/IssueParts";
import { Composer } from "../../components/ReviewParts";
import { IssueState } from "../../gen/pando_platform_pb";
import { Markdown } from "../../markdown";
import { useAddIssueComment, useIssue, useIssueComments, useUpdateIssue } from "../../platform-data";
import { useRepo, useRepoParams } from "../../repo";
import { Author, ErrorState, LoadError, Skeleton, Time } from "../../ui";

/** One issue: what needs doing, the discussion, who is on it, and what fixes it. */
export function IssueView() {
  const { key } = useRepo();
  const rp = useRepoParams();
  const { number: n } = useParams({ from: "/$owner/$repo/issues/$number" });
  const number = BigInt(n);
  const issue = useIssue(key, number);
  const comments = useIssueComments(key, number);
  const update = useUpdateIssue(key, number);
  const comment = useAddIssueComment(key, number);
  const [assigning, setAssigning] = useState(false);
  const [assignees, setAssignees] = useState("");
  if (issue.error) return <LoadError error={issue.error} what={`issue #${n}`} title={`Issue #${n} didn't load`} retry={() => issue.refetch()} />;
  if (!issue.data) return <Skeleton rows={6} />;
  const i = issue.data;
  const open = i.state === IssueState.OPEN;
  return (
    <div className="page page-narrow">
      <header className="issue-head">
        <h1>
          {i.title} <span className="muted">#{String(i.number)}</span>
        </h1>
        <div className="row-meta">
          <span className="pill" data-state={open ? "open" : "closed"}>
            <IssueStateIcon issue={i} /> {issueWord(i.state)}
          </span>
          <Author author={i.author} /> opened it <Time ms={Number(i.createdAtMs)} />
          {i.labels.map((l) => (
            <span key={l} className="tag">
              {l}
            </span>
          ))}
        </div>
      </header>
      <div className="issue-layout">
        <div className="stack">
          <section className="panel">
            <div className="comment">
              <div className="comment-head">
                <Author author={i.author} />
                <Time ms={Number(i.createdAtMs)} />
              </div>
              {i.body.trim() ? <Markdown text={i.body} className="comment-body markdown" /> : <p className="comment-body muted">No description.</p>}
            </div>
            {comments.data?.map((c) => (
              <div className="comment" key={c.id}>
                <div className="comment-head">
                  <Author author={c.author} />
                  <Time ms={Number(c.createdAtMs)} />
                </div>
                <Markdown text={c.body} className="comment-body markdown" />
              </div>
            ))}
          </section>
          <section className="panel">
            <Composer
              key={comments.data?.length}
              label="Add to the discussion"
              submitLabel="Comment"
              pending={comment.isPending}
              error={comment.error ? (comment.error as Error).message : null}
              onSubmit={(body) => comment.mutate(body)}
            />
          </section>
          {update.error && <ErrorState error={update.error} title="That didn't save" />}
        </div>
        <aside className="issue-side" aria-label="Issue details">
          <div>
            <h2>Assignees</h2>
            {assigning ? (
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  update.mutate({ fields: ["assignees"], assignees: assignees.split(/[\s,]+/).filter(Boolean) }, { onSuccess: () => setAssigning(false) });
                }}
              >
                <input className="input" value={assignees} onChange={(e) => setAssignees(e.target.value)} placeholder="mira, agent/fix-01" aria-label="Assignees" autoFocus spellCheck={false} />
                <div className="token-form-actions" style={{ marginTop: 8 }}>
                  <button type="button" className="button button-quiet" onClick={() => setAssigning(false)}>
                    Cancel
                  </button>
                  <button type="submit" className="button" data-primary="">
                    Save
                  </button>
                </div>
              </form>
            ) : (
              <>
                {i.assignees.length ? i.assignees.map((a) => <Author key={a} author={a} />) : <p className="muted">No one yet</p>}
                <button type="button" className="button button-quiet" onClick={() => (setAssignees(i.assignees.join(", ")), setAssigning(true))}>
                  {i.assignees.length ? "Change" : "Assign someone or an agent"}
                </button>
              </>
            )}
          </div>
          <div>
            <h2>Fixed by</h2>
            {i.fixedBy.length ? (
              i.fixedBy.map((cr) => (
                <Link key={String(cr)} to="/$owner/$repo/reviews/$number" params={{ ...rp, number: String(cr) }}>
                  #{String(cr)}
                </Link>
              ))
            ) : (
              <p className="muted">
                A change request that says <span className="mono">Fixes #{String(i.number)}</span> closes this when it lands.
              </p>
            )}
          </div>
          <div>
            {open ? (
              <>
                <button type="button" className="button" disabled={update.isPending} onClick={() => update.mutate({ fields: ["state"], state: IssueState.COMPLETED })}>
                  Close as done
                </button>
                <button type="button" className="button button-quiet" disabled={update.isPending} onClick={() => update.mutate({ fields: ["state"], state: IssueState.NOT_PLANNED })}>
                  Close as not planned
                </button>
              </>
            ) : (
              <button type="button" className="button" disabled={update.isPending} onClick={() => update.mutate({ fields: ["state"], state: IssueState.OPEN })}>
                Reopen
              </button>
            )}
          </div>
        </aside>
      </div>
    </div>
  );
}
