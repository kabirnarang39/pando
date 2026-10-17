import { Link } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { Cell, Column, Row, Table, TableBody, TableHeader, type SortDescriptor } from "react-aria-components";
import { useLocks } from "../data";
import { Author, Empty, ErrorState, Icon, Skeleton, Time } from "../ui";

/** Files that cannot be merged are locked while someone edits them; landing over a lock fails. */
export function LocksView() {
  const locks = useLocks();
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<SortDescriptor>({ column: "at", direction: "descending" });
  const rows = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    const list = (locks.data ?? []).filter((l) => !needle || l.path.toLowerCase().includes(needle) || l.owner.toLowerCase().includes(needle));
    const key = sort.column as "path" | "owner" | "at";
    const dir = sort.direction === "ascending" ? 1 : -1;
    return list.sort((a, b) => (a[key] < b[key] ? -dir : a[key] > b[key] ? dir : 0));
  }, [locks.data, filter, sort]);

  return (
    <div className="page">
      <div className="page-inner">
        <h1>Locks</h1>
        <p className="lede">
          Files that can't be merged, such as designs and images, are locked while someone edits them. A change to a locked file lands
          only for the person holding the lock. Lock with <code>pando lock PATH</code>; release with <code>pando unlock PATH</code>.
        </p>
        {locks.isPending ? (
          <Skeleton />
        ) : locks.error ? (
          <ErrorState error={locks.error} retry={() => locks.refetch()} title="Locks didn't load" />
        ) : locks.data.length === 0 ? (
          <Empty title="No files are locked">When someone runs pando lock, the file shows up here with who holds it and since when.</Empty>
        ) : (
          <>
            <input
              className="filter"
              type="search"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="Filter by path or owner"
              aria-label="Filter locks by path or owner"
              style={{ width: "min(360px, 100%)", marginBottom: 12 }}
            />
            <div className="table-wrap">
              <Table aria-label="Locked files" className="table" sortDescriptor={sort} onSortChange={setSort}>
                <TableHeader>
                  <Column id="path" isRowHeader allowsSorting>
                    {({ sortDirection }) => <SortLabel label="File" dir={sortDirection} />}
                  </Column>
                  <Column id="owner" allowsSorting>
                    {({ sortDirection }) => <SortLabel label="Locked by" dir={sortDirection} />}
                  </Column>
                  <Column id="at" allowsSorting>
                    {({ sortDirection }) => <SortLabel label="Since" dir={sortDirection} />}
                  </Column>
                </TableHeader>
                <TableBody items={rows} renderEmptyState={() => <span className="muted">No locks match.</span>}>
                  {(l) => (
                    <Row id={l.path}>
                      <Cell>
                        <Link to="/code/$" params={{ _splat: l.path }} search={(s) => ({ ref: s.ref })} className="mono">
                          {l.path}
                        </Link>
                      </Cell>
                      <Cell>
                        <Author author={l.owner} />
                      </Cell>
                      <Cell>
                        <Time ms={l.at} />
                      </Cell>
                    </Row>
                  )}
                </TableBody>
              </Table>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function SortLabel({ label, dir }: { label: string; dir?: "ascending" | "descending" }) {
  return (
    <span className="sort">
      {label}
      {dir && (
        <span aria-hidden="true" style={{ display: "inline-flex", transform: `rotate(${dir === "ascending" ? -90 : 90}deg)` }}>
          <Icon name="chevron" size={12} />
        </span>
      )}
    </span>
  );
}
