import { IssueInboxRow } from "../components/IssueParts";
import { Link, Navigate } from "@tanstack/react-router";
import { ChecksIcon, StateIcon } from "../components/StateIcons";
import { EventKind } from "../gen/pando_platform_pb";
import { SINGLE, useCapabilities, useEvents, useInbox, useRepositories, useViewer } from "../platform-data";
import { ago, Author, Empty, ErrorState, Icon, Skeleton, Time, useNow } from "../ui";

function greeting(h: number) {
  return h < 5 ? "Working late" : h < 12 ? "Good morning" : h < 18 ? "Good afternoon" : "Good evening";
}

/** Where every visit starts: what waits on you, your repositories, and what just happened. */
export function HomeView() {
  const viewer = useViewer();
  const inbox = useInbox();
  const repos = useRepositories();
  const events = useEvents();
  const caps = useCapabilities();
  const reviews = caps.data?.reviews ?? false;
  const now = Math.max(useNow(), Date.now());
  const name = viewer.data?.account?.displayName.split(" ")[0];
  const lands = repos.data?.reduce((s, r) => s + r.lands24h, 0) ?? 0;
  // A server that hosts one repository has nothing else to show: go straight to it.
  if (viewer.data && !viewer.data.platform) return <Navigate to="/$owner/$repo" params={{ owner: SINGLE.owner, repo: SINGLE.name }} replace />;
  // Signed in, but in no organisation yet: start setting one up.
  if (viewer.data?.platform && viewer.data.organizations.length === 0 && repos.data?.length === 0) return <Navigate to="/welcome" replace />;

  // Expired token, server down: say so, instead of loading forever.
  const failed = viewer.error ?? repos.error ?? inbox.error;
  if (failed && (!inbox.data || !repos.data))
    return (
      <div className="page">
        <ErrorState error={failed} title="Your repositories didn't load" retry={() => void Promise.all([viewer.refetch(), repos.refetch(), inbox.refetch()])} />
      </div>
    );
  // Appear once, complete: panels filling in one by one push each other around on phones.
  if (!inbox.data || !repos.data || caps.isPending)
    return (
      <div className="page" aria-busy="true">
        <Skeleton rows={14} />
      </div>
    );

  return (
    <div className="page">
      <div className="page-head">
        <div>
          <h1 className="hello">
            {greeting(new Date().getHours())}
            {name ? `, ${name}` : ""}
          </h1>
          <p>
            {!reviews
              ? `${lands.toLocaleString()} changes landed across your repositories today.`
              : inbox.data
              ? inbox.data.length === 0
                ? "Nothing is waiting on you."
                : `${inbox.data.length} ${inbox.data.length === 1 ? "thing is" : "things are"} waiting on you. ${lands.toLocaleString()} changes landed across your repositories today.`
              : "Loading your day…"}
          </p>
        </div>
      </div>
      <div className="grid-2">
        <div className="stack">
          {reviews && (
          <section className="panel" aria-labelledby="waiting-title">
            <div className="panel-head">
              <Icon name="inbox" />
              <h2 id="waiting-title">Waiting on you</h2>
              <Link to="/inbox" className="more-link">
                Inbox
              </Link>
            </div>
            {inbox.isPending ? (
              <Skeleton rows={4} />
            ) : !inbox.data?.length ? (
              <Empty title="You're clear">Review requests and agents ready to land show up here.</Empty>
            ) : (
              <ul className="list">
                {inbox.data.slice(0, 6).map((i) => {
                  if (!i.changeRequest) {
                    return (
                      <li key={`issue-${i.issue?.repository?.name}#${i.issue?.number}`}>
                        <IssueInboxRow item={i} showReason />
                      </li>
                    );
                  }
                  const cr = i.changeRequest;
                  return (
                    <li key={`${cr.repository?.name}#${cr.number}`}>
                      <Link to="/$owner/$repo/reviews/$number" params={{ owner: cr.repository!.owner, repo: cr.repository!.name, number: String(cr.number) }} className="inbox-row">
                        <span className="unread" data-on={i.unread || undefined} aria-label={i.unread ? "new" : undefined} />
                        <StateIcon cr={cr} />
                        <div className="row-main">
                          <div className="row-title">{cr.title}</div>
                          <div className="row-meta">
                            <span className="reason">{i.reason}</span>
                            <span className="mono">
                              {cr.repository?.name}#{String(cr.number)}
                            </span>
                            <Author author={cr.author} />
                          </div>
                        </div>
                        <span className="row-side">
                          <ChecksIcon checks={cr.checks} />
                          <Time ms={Number(cr.updatedAtMs)} />
                        </span>
                      </Link>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
          )}
          <section className="panel" aria-labelledby="repos-title">
            <div className="panel-head">
              <Icon name="repo" />
              <h2 id="repos-title">Repositories</h2>
            </div>
            {repos.isPending ? (
              <Skeleton rows={4} />
            ) : !repos.data?.length ? (
              <Empty title="No repositories yet">
                {viewer.data?.organizations[0] ? (
                  <>
                    Create one on <Link to="/$owner" params={{ owner: viewer.data.organizations[0].login }}>{viewer.data.organizations[0].displayName || viewer.data.organizations[0].login}</Link>.
                  </>
                ) : (
                  "Repositories you can read show up here."
                )}
              </Empty>
            ) : (
              <div className="table-wrap">
                <table className="table">
                  <thead>
                    <tr>
                      <th>Repository</th>
                      <th className="hide-small">Landed today</th>
                      <th>Open reviews</th>
                      <th className="hide-small">Last landing</th>
                    </tr>
                  </thead>
                  <tbody>
                    {repos.data?.map((r) => (
                      <tr key={r.name}>
                        <td>
                          <Link to="/$owner/$repo" params={{ owner: r.owner, repo: r.name }} className="repo-card-name">
                            {r.owner}/{r.name}
                          </Link>
                          <div className="muted" style={{ fontSize: "var(--t-xs)", maxWidth: "52ch" }}>
                            {r.description}
                          </div>
                        </td>
                        <td className="num hide-small">{r.lands24h.toLocaleString()}</td>
                        <td className="num">{r.openChangeRequests}</td>
                        <td className="muted hide-small">
                          <span className="row-side" style={{ gap: 6 }}>
                            <span className="pulse" data-live={now - Number(r.tipAtMs) < 90_000 || undefined} />
                            {ago(Number(r.tipAtMs), now)}
                          </span>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </section>
        </div>
        <section className="panel" aria-labelledby="activity-title">
          <div className="panel-head">
            <Icon name="activity" />
            <h2 id="activity-title">Activity</h2>
          </div>
          {events.isPending ? (
            <Skeleton rows={8} />
          ) : !events.data?.length ? (
            <Empty title="Nothing has happened yet">Landings, reviews and comments in your repositories show up here.</Empty>
          ) : (
            <div>
              {events.data?.slice(0, 18).map((e) => (
                <div className="activity" key={e.id}>
                  <Author author={e.actor} compact />
                  <span className="what">
                    <strong>{e.actor.split("/").pop()}</strong> {e.summary.replace(/^(opened|landed|commented on|approved|requested changes on|reviewed|closed|created) /, (m) => m)}
                    {e.kind !== EventKind.REPOSITORY_CREATED && <span className="muted"> in {e.repository?.name}</span>}
                  </span>
                  <span className="muted" style={{ fontSize: "var(--t-xs)" }}>
                    <Time ms={Number(e.atMs)} />
                  </span>
                </div>
              ))}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
