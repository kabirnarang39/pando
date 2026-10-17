import { useState } from "react";
import { Button, Dialog, DialogTrigger, Popover, Tooltip, TooltipTrigger } from "react-aria-components";
import { hex } from "../api/client";
import { ChangeRequestState, CheckState, Verdict, type ChangeRequest, type Check, type Comment } from "../gen/pando_platform_pb";
import { Markdown } from "../markdown";
import { useAddComment, useResolveThread, useSubmitReview } from "../platform-data";
import { Author, Icon, Time } from "../ui";

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

export function StatePill({ cr }: { cr: ChangeRequest }) {
  const s = stateOf(cr);
  const tone = s === "open" ? "ok" : s === "landed" ? "warn" : s === "closed" ? "bad" : undefined;
  return (
    <span className="pill" data-tone={tone}>
      <Icon name={s === "landed" ? "merge" : s === "closed" ? "x" : "review"} size={13} />
      {s === "open" ? "Open" : s === "landed" ? "Landed" : s === "closed" ? "Closed" : "Draft"}
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

export function CheckPill({ check }: { check: Check }) {
  const tone = check.state === CheckState.FAILED ? "bad" : check.state === CheckState.PENDING ? "warn" : check.state === CheckState.PASSED ? "ok" : undefined;
  const icon = check.state === CheckState.FAILED ? "fail" : check.state === CheckState.PENDING ? "pending" : "ok";
  return (
    <TooltipTrigger delay={300}>
      <Button className="pill" data-tone={tone} style={{ cursor: "default" }}>
        <Icon name={icon} size={13} />
        {check.name}
      </Button>
      <Tooltip className="tooltip" offset={6}>
        {check.state === CheckState.FAILED ? check.summary || "Failed" : check.state === CheckState.PENDING ? "Running" : "Passed"}
      </Tooltip>
    </TooltipTrigger>
  );
}

/** Approvals that still count (made on the current tip) and requests for changes. */
export function reviewState(cr: ChangeRequest) {
  const tip = hex(cr.sourceTip);
  const approved = cr.reviews.filter((r) => r.verdict === Verdict.APPROVED && hex(r.reviewedTip) === tip);
  const stale = cr.reviews.filter((r) => r.verdict === Verdict.APPROVED && hex(r.reviewedTip) !== tip);
  const changes = cr.reviews.filter((r) => r.verdict === Verdict.CHANGES_REQUESTED);
  return { approved, stale, changes };
}

export function ReviewBadge({ cr }: { cr: ChangeRequest }) {
  const { approved, changes } = reviewState(cr);
  if (changes.length)
    return (
      <span className="pill" data-tone="bad">
        Changes requested
      </span>
    );
  if (approved.length)
    return (
      <span className="pill" data-tone="ok">
        <Icon name="check" size={12} /> Approved
      </span>
    );
  return null;
}

/** Submit a review: comment, approve or request changes. */
export function ReviewButton({ repo, cr }: { repo: string; cr: ChangeRequest }) {
  const [open, setOpen] = useState(false);
  const [verdict, setVerdict] = useState<Verdict>(Verdict.APPROVED);
  const [body, setBody] = useState("");
  const submit = useSubmitReview(repo, cr.number);
  const choices: [Verdict, string, string][] = [
    [Verdict.APPROVED, "Approve", "It can land once checks pass."],
    [Verdict.CHANGES_REQUESTED, "Request changes", "It can't land until you approve a new version."],
    [Verdict.COMMENTED, "Comment", "Feedback without a decision."],
  ];
  return (
    <DialogTrigger isOpen={open} onOpenChange={setOpen}>
      <Button id="review-button" className="button" isDisabled={cr.state !== ChangeRequestState.OPEN}>
        <Icon name="eye" size={15} /> Review
      </Button>
      <Popover placement="bottom end" offset={6}>
        <Dialog className="verdict-menu" aria-labelledby="verdict-title">
          <form
            onSubmit={(e) => {
              e.preventDefault();
              submit.mutate({ verdict, body }, { onSuccess: () => (setOpen(false), setBody("")) });
            }}
          >
            <h2 id="verdict-title">Your review</h2>
            <textarea className="input" style={{ width: "100%" }} value={body} onChange={(e) => setBody(e.target.value)} placeholder="Leave a note (optional)" aria-label="Review note" />
            <div className="verdicts" role="radiogroup" aria-label="Decision">
              {choices.map(([v, label, help]) => (
                <label key={v}>
                  <input type="radio" name="verdict" checked={verdict === v} onChange={() => setVerdict(v)} />
                  <strong>{label}</strong>
                  <small>{help}</small>
                </label>
              ))}
            </div>
            {submit.error && <p className="status" data-kind="error">{(submit.error as Error).message}</p>}
            <div className="row-actions" style={{ justifyContent: "flex-end" }}>
              <button type="button" className="button button-quiet" onClick={() => setOpen(false)}>
                Cancel
              </button>
              <button type="submit" className="button" data-primary="" disabled={submit.isPending || (verdict !== Verdict.APPROVED && !body.trim())}>
                {submit.isPending ? "Submitting…" : "Submit review"}
              </button>
            </div>
          </form>
        </Dialog>
      </Popover>
    </DialogTrigger>
  );
}

/** A comment thread (on a line or on the change request), with reply and resolve. */
export function Thread({ repo, cr, comments, at, onDone }: { repo: string; cr: ChangeRequest; comments: Comment[]; at?: { path: string; line: number }; onDone?(): void }) {
  const [reply, setReply] = useState("");
  const add = useAddComment(repo, cr.number);
  const resolve = useResolveThread(repo, cr.number);
  const head = comments[0];
  const resolved = !!head?.resolved;
  return (
    <div className="thread" data-resolved={resolved || undefined} data-inline={at || head?.path ? "" : undefined}>
      <div className="thread-inner">
        {comments.map((c) => (
          <div className="comment" key={c.id}>
            <div className="comment-head">
              <Author author={c.author} />
              <Time ms={Number(c.createdAtMs)} />
              {resolved && c === head && <span className="pill">Resolved</span>}
            </div>
            <Markdown text={c.body} className="comment-body markdown" />
          </div>
        ))}
        <form
          className="thread-foot"
          onSubmit={(e) => {
            e.preventDefault();
            add.mutate({ body: reply, path: head?.path ?? at?.path, line: head?.line ?? at?.line, thread: head?.thread }, { onSuccess: () => (setReply(""), onDone?.()) });
          }}
        >
          <input className="input" style={{ flex: 1, minWidth: 160 }} value={reply} onChange={(e) => setReply(e.target.value)} placeholder={head ? "Reply" : "Write a comment"} aria-label={head ? "Reply" : "Comment"} autoFocus={!head && !!at} />
          <button type="submit" className="button" data-primary="" disabled={!reply.trim() || add.isPending}>
            {head ? "Reply" : "Comment"}
          </button>
          {head && (
            <button type="button" className="button button-quiet" onClick={() => resolve.mutate({ thread: head.thread, resolved: !resolved })}>
              {resolved ? "Reopen" : "Resolve"}
            </button>
          )}
          {!head && onDone && (
            <button type="button" className="button button-quiet" onClick={onDone}>
              Cancel
            </button>
          )}
        </form>
      </div>
    </div>
  );
}
