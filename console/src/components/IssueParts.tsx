import { Link } from "@tanstack/react-router";
import { IssueState, type InboxItem, type Issue } from "../gen/pando_platform_pb";
import { Author, Icon, Time } from "../ui";

/** Open, done, or not planned, as an icon with its name for screen readers. */
export function IssueStateIcon({ issue }: { issue: Issue }) {
  if (issue.state === IssueState.OPEN) return <span className="issue-state" data-state="open"><Icon name="dot" size={16} label="Open issue" /></span>;
  if (issue.state === IssueState.COMPLETED) return <span className="issue-state" data-state="done"><Icon name="ok" size={16} label="Done" /></span>;
  return <span className="issue-state" data-state="dropped"><Icon name="x" size={16} label="Not planned" /></span>;
}

export const issueWord = (s: IssueState) => (s === IssueState.OPEN ? "Open" : s === IssueState.COMPLETED ? "Done" : "Not planned");

/** An issue in the inbox or on the home page. */
export function IssueInboxRow({ item, showReason }: { item: InboxItem; showReason?: boolean }) {
  const i = item.issue!;
  return (
    <Link to="/$owner/$repo/issues/$number" params={{ owner: i.repository!.owner, repo: i.repository!.name, number: String(i.number) }} className="inbox-row">
      <span className="unread" />
      <IssueStateIcon issue={i} />
      <div className="row-main">
        <div className="row-title">{i.title}</div>
        <div className="row-meta">
          {showReason && <span className="reason">{item.reason}</span>}
          <span className="mono">
            {i.repository?.name}#{String(i.number)}
          </span>
          <Author author={i.author} />
        </div>
      </div>
      <span className="row-side">
        <Time ms={Number(i.updatedAtMs)} />
      </span>
    </Link>
  );
}
