import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { IssueStateIcon } from "../../components/IssueParts";
import { IssueState } from "../../gen/pando_platform_pb";
import { useCreateIssue, useIssues } from "../../platform-data";
import { useRepo, useRepoParams } from "../../repo";
import { Author, Empty, ErrorState, Icon, Skeleton, Time } from "../../ui";

const OPEN = [IssueState.OPEN];
const CLOSED = [IssueState.COMPLETED, IssueState.NOT_PLANNED];

/** Work to do, and who (or which agent) is on it. */
export function IssuesView() {
  const { key } = useRepo();
  const rp = useRepoParams();
  const [closed, setClosed] = useState(false);
  const [query, setQuery] = useState("");
  const [creating, setCreating] = useState(false);
  const open = useIssues(key, OPEN);
  const done = useIssues(key, CLOSED);
  const list = (closed ? done : open).data ?? [];
  const q = query.toLowerCase();
  const shown = list.filter((i) => !q || i.title.toLowerCase().includes(q) || i.assignees.some((a) => a.includes(q)) || i.labels.some((l) => l.includes(q)));
  const current = closed ? done : open;
  return (
    <div className="page">
      <div className="toolbar-row">
        <div className="segmented" role="group" aria-label="Which issues">
          <button type="button" aria-pressed={!closed} onClick={() => setClosed(false)}>
            Open <span className="count">{open.data?.length ?? 0}</span>
          </button>
          <button type="button" aria-pressed={closed} onClick={() => setClosed(true)}>
            Closed <span className="count">{done.data?.length ?? 0}</span>
          </button>
        </div>
        <div className="query grow" data-plain="" role="search">
          <Icon name="search" size={15} />
          <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Filter by title, assignee or label" aria-label="Filter issues" />
        </div>
        {!creating && (
          <button type="button" className="button" data-primary="" onClick={() => setCreating(true)}>
            <Icon name="plus" size={15} /> New issue
          </button>
        )}
      </div>
      {creating && <NewIssue onDone={() => setCreating(false)} />}
      <div className="panel">
        {current.isPending ? (
          <Skeleton rows={6} />
        ) : current.error ? (
          <ErrorState error={current.error} retry={() => current.refetch()} />
        ) : shown.length === 0 ? (
          <Empty title={query ? "No issues match" : closed ? "Nothing closed yet" : "No open issues"}>
            {query ? "Clear the filter to see every issue." : "Open one for work to do; assign it to a person or an agent."}
          </Empty>
        ) : (
          <ul className="list" aria-label="Issues">
            {shown.map((i) => (
              <li key={String(i.number)}>
                <Link to="/$owner/$repo/issues/$number" params={{ ...rp, number: String(i.number) }} className="row" style={{ gridTemplateColumns: "auto minmax(0,1fr) auto" }}>
                  <IssueStateIcon issue={i} />
                  <div className="row-main">
                    <div className="row-title">{i.title}</div>
                    <div className="row-meta">
                      <span className="mono">#{String(i.number)}</span>
                      <Author author={i.author} />
                      {i.labels.map((l) => (
                        <span key={l} className="tag">
                          {l}
                        </span>
                      ))}
                    </div>
                  </div>
                  <span className="row-side">
                    {i.assignees.map((a) => (
                      <Author key={a} author={a} compact />
                    ))}
                    {i.comments > 0 && (
                      <span className="num" aria-label={`${i.comments} comments`}>
                        <Icon name="comment" size={13} /> {i.comments}
                      </span>
                    )}
                    <Time ms={Number(i.updatedAtMs)} />
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function NewIssue({ onDone }: { onDone(): void }) {
  const { key } = useRepo();
  const create = useCreateIssue(key);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [assignees, setAssignees] = useState("");
  const [labels, setLabels] = useState("");
  const words = (s: string) => s.split(/[\s,]+/).filter(Boolean);
  return (
    <form
      className="panel token-form"
      aria-label="New issue"
      onSubmit={(e) => {
        e.preventDefault();
        create.mutate({ title: title.trim(), body, assignees: words(assignees), labels: words(labels) }, { onSuccess: onDone });
      }}
    >
      <label>
        <span>Title</span>
        <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="What needs doing" required maxLength={300} autoFocus />
      </label>
      <label>
        <span>Description</span>
        <textarea className="input composer-text" value={body} onChange={(e) => setBody(e.target.value)} rows={4} placeholder="What, why, and how to tell it's done. Markdown works." />
      </label>
      <div className="token-form-row">
        <label>
          <span>Assign to</span>
          <input className="input" value={assignees} onChange={(e) => setAssignees(e.target.value)} placeholder="mira, agent/fix-01" spellCheck={false} />
        </label>
        <label>
          <span>Labels</span>
          <input className="input" value={labels} onChange={(e) => setLabels(e.target.value)} placeholder="bug, billing" spellCheck={false} />
        </label>
      </div>
      {create.error && (
        <p className="status" data-kind="error" role="alert">
          {(create.error as Error).message}
        </p>
      )}
      <div className="token-form-actions">
        <button type="button" className="button button-quiet" onClick={onDone}>
          Cancel
        </button>
        <button type="submit" className="button" data-primary="" disabled={!title.trim() || create.isPending}>
          {create.isPending ? "Opening…" : "Open issue"}
        </button>
      </div>
    </form>
  );
}
