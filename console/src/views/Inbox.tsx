import { Link } from "@tanstack/react-router";
import { ChecksIcon, ReviewBadge, StateIcon } from "../components/ReviewParts";
import { useInbox } from "../platform-data";
import { Author, Empty, ErrorState, Skeleton, Time } from "../ui";

/** Everything that waits on you, grouped by why. */
export function InboxView() {
  const inbox = useInbox();
  const groups = new Map<string, NonNullable<typeof inbox.data>>();
  for (const i of inbox.data ?? []) groups.set(i.reason, [...(groups.get(i.reason) ?? []), i]);
  return (
    <div className="page page-narrow">
      <div className="page-head">
        <div>
          <h1>Inbox</h1>
          <p>Reviews requested from you, and your agents' changes that are ready to land. It updates as things change.</p>
        </div>
      </div>
      {inbox.isPending ? (
        <Skeleton rows={8} />
      ) : inbox.error ? (
        <ErrorState error={inbox.error} retry={() => inbox.refetch()} />
      ) : groups.size === 0 ? (
        <div className="panel">
          <Empty title="You're clear">Nothing is waiting on you. New review requests appear here as they arrive.</Empty>
        </div>
      ) : (
        <div className="stack">
          {[...groups.entries()].map(([reason, items]) => (
            <section className="panel" key={reason} aria-label={reason}>
              <div className="panel-head">
                <h2>{reason}</h2>
                <span className="count">{items.length}</span>
              </div>
              <ul className="list">
                {items.map((i) => {
                  const cr = i.changeRequest!;
                  return (
                    <li key={`${cr.repository?.name}#${cr.number}`}>
                      <Link to="/$owner/$repo/reviews/$number" params={{ owner: cr.repository!.owner, repo: cr.repository!.name, number: String(cr.number) }} className="inbox-row">
                        <span className="unread" data-on={i.unread || undefined} aria-label={i.unread ? "new" : undefined} />
                        <StateIcon cr={cr} />
                        <div className="row-main">
                          <div className="row-title">{cr.title}</div>
                          <div className="row-meta">
                            <span className="mono">
                              {cr.repository?.name}#{String(cr.number)}
                            </span>
                            <Author author={cr.author} />
                            {cr.blockers[0] && <span>{cr.blockers[0]}</span>}
                          </div>
                        </div>
                        <span className="row-side">
                          <ReviewBadge cr={cr} />
                          <ChecksIcon checks={cr.checks} />
                          <Time ms={Number(cr.updatedAtMs)} />
                        </span>
                      </Link>
                    </li>
                  );
                })}
              </ul>
            </section>
          ))}
        </div>
      )}
    </div>
  );
}
