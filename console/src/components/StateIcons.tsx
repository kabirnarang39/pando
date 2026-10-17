import { ChangeRequestState, CheckState, type ChangeRequest, type Check } from "../gen/pando_platform_pb";
import { Icon } from "../ui";

const stateName: Record<number, "open" | "draft" | "landed" | "closed"> = {
  [ChangeRequestState.OPEN]: "open",
  [ChangeRequestState.DRAFT]: "draft",
  [ChangeRequestState.LANDED]: "landed",
  [ChangeRequestState.CLOSED]: "closed",
};
export const stateOf = (cr: ChangeRequest) => stateName[cr.state] ?? "open";

export function StateIcon({ cr, size = 16 }: { cr: ChangeRequest; size?: number }) {
  const s = stateOf(cr);
  return (
    <span className="state-icon" data-state={s} title={s[0]!.toUpperCase() + s.slice(1)}>
      <Icon name={s === "landed" ? "merge" : s === "closed" ? "x" : "review"} size={size} label={s} />
    </span>
  );
}

/** One icon for all checks: failing beats running beats passing. */
export function ChecksIcon({ checks }: { checks: Check[] }) {
  if (!checks.length) return null;
  const failed = checks.filter((c) => c.state === CheckState.FAILED).length;
  const pending = checks.filter((c) => c.state === CheckState.PENDING).length;
  const passed = checks.filter((c) => c.state === CheckState.PASSED).length;
  const label = failed ? `${failed} of ${checks.length} checks failed` : pending ? `${pending} checks running` : `All ${passed} checks passed`;
  return (
    <span title={label} style={{ display: "inline-flex", color: failed ? "var(--bad)" : pending ? "var(--warn)" : "var(--ok)" }}>
      <Icon name={failed ? "fail" : pending ? "pending" : "ok"} size={15} label={label} />
    </span>
  );
}

