import { Link } from "@tanstack/react-router";
import { asText, Author, Empty, Icon, Skeleton, Time } from "../../ui";
import { FleetLanes } from "../../components/Fleet";
import { ChecksIcon, ReviewBadge, StateIcon } from "../../components/ReviewParts";
import { title } from "../../api/client";
import { useDir, useFile, useLive, useLocks } from "../../data";
import { ChangeRequestState } from "../../gen/pando_platform_pb";
import { Markdown } from "../../markdown";
import { useCapabilities, useChangeRequests } from "../../platform-data";
import { useRepo, useRepoParams } from "../../repo";
import { useRefName } from "../../shell/hooks";
import { useRepoUrl } from "./RepoLayout";
import { CopyCommand } from "../../components/CopyCommand";

/** A repository at a glance: the fleet at work, what waits for review, the README. */
export function OverviewView() {
  const { key } = useRepo();
  const rp = useRepoParams();
  const ref = useRefName();
  const live = useLive(ref);
  const crs = useChangeRequests(key, [ChangeRequestState.OPEN]);
  const locks = useLocks();
  const reviews = useCapabilities().data?.reviews ?? false;
  return (
    <div className="page">
      <div className="grid-2">
        <div className="stack">
          {live.tip.data === null ? (
            <EmptyRepo />
          ) : (
          <section className="panel" aria-labelledby="fleet-title">
            <div className="panel-head">
              <Icon name="activity" />
              <h2 id="fleet-title">The fleet, last hour</h2>
              <Link to="/$owner/$repo/agents" params={rp} className="more-link">
                All agents
              </Link>
            </div>
            <FleetLanes changes={live.changes} minutes={60} max={8} />
          </section>
          )}
          <Readme />
        </div>
        <div className="stack">
          {reviews && (
          <section className="panel" aria-labelledby="waiting-title">
            <div className="panel-head">
              <Icon name="review" />
              <h2 id="waiting-title">Waiting for review</h2>
              <Link to="/$owner/$repo/reviews" params={rp} className="more-link">
                All
              </Link>
            </div>
            {crs.isPending ? (
              <Skeleton rows={4} />
            ) : !crs.data?.length ? (
              <Empty title="Nothing is waiting" />
            ) : (
              <ul className="list">
                {crs.data.slice(0, 6).map((cr) => (
                  <li key={String(cr.number)}>
                    <Link to="/$owner/$repo/reviews/$number" params={{ ...rp, number: String(cr.number) }} className="row">
                      <StateIcon cr={cr} />
                      <div className="row-main">
                        <div className="row-title">{cr.title}</div>
                        <div className="row-meta">
                          <span className="num">#{String(cr.number)}</span>
                          <Author author={cr.author} />
                        </div>
                      </div>
                      <div className="row-side">
                        <ReviewBadge cr={cr} />
                        <ChecksIcon checks={cr.checks} />
                      </div>
                    </Link>
                  </li>
                ))}
              </ul>
            )}
          </section>
          )}
          <section className="panel" aria-labelledby="recent-title">
            <div className="panel-head">
              <Icon name="changes" />
              <h2 id="recent-title">Just landed</h2>
              <Link to="/$owner/$repo/changes" params={rp} className="more-link">
                All
              </Link>
            </div>
            {!live.changes ? (
              <Skeleton rows={5} />
            ) : !live.changes.length ? (
              <Empty title={`Nothing has landed on ${ref} yet`} />
            ) : (
              <ul className="list">
                {live.changes.slice(0, 7).map((c) => (
                  <li key={c.id}>
                    <Link to="/$owner/$repo/changes" params={rp} search={{ c: c.id }} className="row" style={{ gridTemplateColumns: "auto minmax(0,1fr) auto" }}>
                      <Author author={c.author} compact />
                      <div className="row-title" style={{ fontWeight: 500 }}>
                        {title(c.message)}
                      </div>
                      <span className="row-side">
                        <Time ms={c.time} />
                      </span>
                    </Link>
                  </li>
                ))}
              </ul>
            )}
          </section>
          {locks.data && locks.data.length > 0 && (
            <section className="panel" aria-labelledby="locks-title">
              <div className="panel-head">
                <Icon name="lock" />
                <h2 id="locks-title">Locked files</h2>
              </div>
              <ul className="list">
                {locks.data.map((l) => (
                  <li key={l.path}>
                    <Link to="/$owner/$repo/code/$" params={{ ...rp, _splat: l.path }} className="row" style={{ gridTemplateColumns: "minmax(0,1fr) auto" }}>
                      <span className="mono row-title" style={{ fontWeight: 500, fontSize: "var(--t-xs)" }}>
                        {l.path}
                      </span>
                      <Author author={l.owner} compact />
                    </Link>
                  </li>
                ))}
              </ul>
            </section>
          )}
        </div>
      </div>
    </div>
  );
}

/** A repository with no history yet: how to put code in it. */
function EmptyRepo() {
  const { name } = useRepo();
  const url = useRepoUrl();
  return (
    <section className="panel" aria-labelledby="empty-title">
      <div className="panel-head">
        <Icon name="repo" />
        <h2 id="empty-title">This repository is empty</h2>
      </div>
      <div className="panel-body commands">
        <CopyCommand label="Start on your computer" cmd={`pando clone ${url} ${name}`} note={`Then add files in ${name}/ and run: pando commit -m "First commit" && pando land. (On a server with sign-in, run pando login first.)`} />
        <CopyCommand label="Bring a git repository" cmd={`pando migrate https://github.com/OWNER/NAME --to ${url}`} note="Full history; from GitHub also open pull requests and issues. Any URL git can clone works." />
      </div>
    </section>
  );
}

function Readme() {
  const ref = useRefName();
  const live = useLive(ref);
  const tip = live.tip.data ?? undefined;
  const root = useDir(tip, "");
  const name = root.data?.find((e) => /^readme(\.md)?$/i.test(e.name))?.name;
  const file = useFile(tip, name ?? "", !!name);
  const text = file.data ? asText(file.data.bytes) : null;
  if (!name) return null;
  return (
    <section className="panel" aria-labelledby="readme-title">
      <div className="panel-head">
        <Icon name="file" />
        <h2 id="readme-title">{name}</h2>
      </div>
      <div className="panel-body" style={{ padding: "18px 22px" }}>
        {text === null ? <Skeleton rows={6} /> : <Markdown text={text} />}
      </div>
    </section>
  );
}
