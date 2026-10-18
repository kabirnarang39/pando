import { Link } from "@tanstack/react-router";
import { FleetLanes } from "../../components/Fleet";
import { useLive } from "../../data";
import { ChangeRequestState } from "../../gen/pando_platform_pb";
import { useChangeRequests, useMembers } from "../../platform-data";
import { useRepo } from "../../repo";
import { useRefName } from "../../shell/hooks";
import { ago, Author, Icon, parseAuthor, Skeleton, useNow } from "../../ui";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Who is landing in this repository: a lane per author, and a table to compare them. */
export function AgentsView() {
  const { key, owner } = useRepo();
  const ref = useRefName();
  const live = useLive(ref);
  const members = useMembers(owner);
  const crs = useChangeRequests(key);
  const now = Math.max(useNow(), Date.now());
  const hour = (live.changes ?? []).filter((c) => c.time > now - 3600_000);
  const authors = new Map<string, { lands: number; last: number }>();
  for (const c of live.changes ?? []) {
    const a = authors.get(c.author) ?? { lands: 0, last: 0 };
    if (c.time > now - 3600_000) a.lands++;
    a.last = Math.max(a.last, c.time);
    authors.set(c.author, a);
  }
  const owners = new Map(members.data?.map((m) => [m.account!.login, m.account!.owner]));
  const rows = [...authors.entries()].sort((a, b) => b[1].lands - a[1].lands || b[1].last - a[1].last);

  return (
    <div className="page">
      <section className="panel" aria-labelledby="lanes-title" style={{ marginBottom: 20 }}>
        <div className="panel-head">
          <Icon name="activity" />
          <h2 id="lanes-title">Last hour</h2>
          <span className="muted" style={{ marginLeft: "auto", fontSize: "var(--t-xs)" }}>
            {plural(hour.length, "landing")} from {plural(new Set(hour.map((c) => c.author)).size, "author")}
          </span>
        </div>
        <FleetLanes changes={live.changes} minutes={60} max={16} />
      </section>
      <section className="panel" aria-labelledby="table-title">
        <div className="panel-head">
          <Icon name="users" />
          <h2 id="table-title">Who lands here</h2>
        </div>
        {!live.changes ? (
          <Skeleton />
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Author</th>
                  <th>Answers to</th>
                  <th>Landed, last hour</th>
                  <th>Open reviews</th>
                  <th>Last landing</th>
                </tr>
              </thead>
              <tbody>
                {rows.map(([author, a]) => {
                  const open = (crs.data ?? []).filter((c) => c.author === author && c.state === ChangeRequestState.OPEN).length;
                  return (
                    <tr key={author}>
                      <td>
                        <Author author={author} />
                      </td>
                      <td>{parseAuthor(author).agent ? owners.get(author) ? <Author author={owners.get(author)!} /> : <span className="muted">Unknown</span> : <span className="muted" aria-label="Not an agent">–</span>}</td>
                      <td className="num">{a.lands}</td>
                      <td className="num">{open > 0 ? <Link to="/$owner/$repo/reviews" params={{ owner, repo: key.split("/")[1]! }} search={{ q: `author:${author}`, view: "open" }}>{open}</Link> : 0}</td>
                      <td className="muted">{ago(a.last, now)}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}
