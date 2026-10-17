import { useRef, useState } from "react";
import { Button, Dialog, DialogTrigger, Popover, Tooltip, TooltipTrigger } from "react-aria-components";
import { hex } from "../api/client";
import { ChangeRequestState, CheckState, Verdict, type ChangeRequest, type Check, type Comment } from "../gen/pando_platform_pb";
import { Markdown } from "../markdown";
import { useAddComment, useResolveThread, useSubmitReview } from "../platform-data";
import { Author, Icon, Time } from "../ui";
import { stateOf } from "./StateIcons";

// The small icons live apart so lists (like Home) do not load the review editor.
export { ChecksIcon, StateIcon, stateOf } from "./StateIcons";

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

/** Where a new comment goes: a file and a line, or a range of lines. */
export interface CommentTarget {
  path: string;
  line: number;
  endLine?: number;
  /** The current text of the commented lines, offered as the start of a suggestion. */
  original?: string;
}

const SUGGESTION = /```suggestion\n([\s\S]*?)```/;
const SUGGESTIONS = new RegExp(SUGGESTION.source, "g");

/**
 * Writing a comment: Markdown with a preview, a suggested change for commented lines,
 * ⌘/Ctrl+Enter to send and Escape to cancel.
 */
export function Composer({
  label,
  submitLabel,
  original,
  autoFocus,
  pending,
  error,
  onSubmit,
  onCancel,
}: {
  label: string;
  submitLabel: string;
  original?: string;
  autoFocus?: boolean;
  pending?: boolean;
  error?: string | null;
  onSubmit(body: string, suggestion: string): void;
  onCancel?(): void;
}) {
  const [body, setBody] = useState("");
  const [preview, setPreview] = useState(false);
  const text = useRef<HTMLTextAreaElement>(null);
  const send = () => {
    const text = body.trim();
    if (!text || pending) return;
    const s = text.match(SUGGESTION);
    onSubmit(text, s ? s[1]!.replace(/\n+$/, "") : "");
  };
  return (
    <form
      className="composer"
      onSubmit={(e) => {
        e.preventDefault();
        send();
      }}
    >
      <div className="composer-head">
        <span className="composer-label">{label}</span>
        <div className="segmented" role="group" aria-label="Write or preview">
          <button type="button" aria-pressed={!preview} onClick={() => setPreview(false)}>
            Write
          </button>
          <button type="button" aria-pressed={preview} onClick={() => setPreview(true)}>
            Preview
          </button>
        </div>
      </div>
      {preview ? (
        <div className="composer-preview">{body.trim() ? <Markdown text={body} /> : <span className="muted">Nothing to preview.</span>}</div>
      ) : (
        <textarea
          ref={text}
          className="input composer-text"
          value={body}
          autoFocus={autoFocus}
          onChange={(e) => setBody(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              send();
            }
            if (e.key === "Escape" && onCancel) {
              e.preventDefault();
              onCancel();
            }
          }}
          placeholder="Leave a comment. Markdown works."
          aria-label={label}
          rows={3}
        />
      )}
      {error && (
        <p className="status" data-kind="error" role="alert">
          {error}
        </p>
      )}
      <div className="composer-foot">
        {original !== undefined && (
          <button
            type="button"
            className="button button-quiet"
            onClick={() => {
              const next = `${body}${body && !body.endsWith("\n") ? "\n" : ""}\`\`\`suggestion\n${original}\n\`\`\`\n`;
              setPreview(false);
              setBody(next);
              // Back to the text, with the caret at the end of the proposed lines.
              const caret = next.length - "\n```\n".length;
              setTimeout(() => (text.current?.focus(), text.current?.setSelectionRange(caret, caret)));
            }}
            title="Propose replacement text for these lines"
          >
            <Icon name="code" size={14} /> Suggest a change
          </button>
        )}
        <span className="muted composer-hint">
          <kbd>⌘</kbd>
          <kbd>↵</kbd> to send
        </span>
        {onCancel && (
          <button type="button" className="button button-quiet" onClick={onCancel}>
            Cancel
          </button>
        )}
        <button type="submit" className="button" data-primary="" disabled={!body.trim() || pending}>
          {pending ? "Sending…" : submitLabel}
        </button>
      </div>
    </form>
  );
}

function lines(c: Comment) {
  if (!c.path) return null;
  return c.endLine && c.endLine > c.line ? `lines ${c.line}–${c.endLine}` : `line ${c.line}`;
}

/** A suggestion as a small diff: what the lines are now, and what the reviewer proposes. */
function Suggestion({ text, original }: { text: string; original?: string }) {
  return (
    <div className="suggestion" aria-label="Suggested change">
      <div className="suggestion-head">
        <Icon name="code" size={13} /> Suggested change
      </div>
      {original !== undefined &&
        original.split("\n").map((l, i) => (
          <div className="drow" data-op="del" key={`o${i}`}>
            <span className="dm" aria-label="removed">−</span>
            <span className="dsrc">{l || " "}</span>
          </div>
        ))}
      {text.split("\n").map((l, i) => (
        <div className="drow" data-op="add" key={`n${i}`}>
          <span className="dm" aria-label="added">+</span>
          <span className="dsrc">{l || " "}</span>
        </div>
      ))}
    </div>
  );
}

/** A comment thread (on lines of a file, or on the change request), with reply and resolve. */
export function Thread({
  repo,
  cr,
  comments,
  at,
  original,
  onDone,
}: {
  repo: string;
  cr: ChangeRequest;
  comments: Comment[];
  /** For a new thread: where it goes. */
  at?: CommentTarget;
  /** Current text of the commented lines (for suggestions). */
  original?: string;
  onDone?(): void;
}) {
  const [replying, setReplying] = useState(false);
  const add = useAddComment(repo, cr.number);
  const resolve = useResolveThread(repo, cr.number);
  const head = comments[0];
  const resolved = !!head?.resolved;
  const inline = !!(at || head?.path);
  const [collapsed, setCollapsed] = useState(resolved);
  const submit = (body: string, suggestion: string) =>
    add.mutate(
      { body, suggestion, path: head?.path ?? at?.path, line: head?.line ?? at?.line, endLine: head?.endLine ?? at?.endLine, thread: head?.thread },
      { onSuccess: () => (setReplying(false), onDone?.()) },
    );

  if (!head && at)
    return (
      <div className="thread" data-inline="">
        <div className="thread-inner">
          <Composer
            label={`Comment on ${at.endLine && at.endLine > at.line ? `lines ${at.line}–${at.endLine}` : `line ${at.line}`} of ${at.path.split("/").pop()}`}
            submitLabel="Comment"
            original={at.original}
            autoFocus
            pending={add.isPending}
            error={add.error ? (add.error as Error).message : null}
            onSubmit={submit}
            onCancel={onDone}
          />
        </div>
      </div>
    );

  if (!head)
    return (
      <div className="thread">
        <div className="thread-inner">
          <Composer label="Add to the conversation" submitLabel="Comment" pending={add.isPending} error={add.error ? (add.error as Error).message : null} onSubmit={submit} />
        </div>
      </div>
    );

  return (
    <div className="thread" data-resolved={resolved || undefined} data-inline={inline ? "" : undefined}>
      <div className="thread-inner">
        {resolved && collapsed ? (
          <button type="button" className="thread-collapsed" onClick={() => setCollapsed(false)}>
            <Icon name="check" size={13} /> Resolved conversation{lines(head) ? ` on ${lines(head)}` : ""}, {comments.length} {comments.length === 1 ? "comment" : "comments"}. Show
          </button>
        ) : (
          <>
            {comments.map((c) => (
              <div className="comment" key={c.id}>
                <div className="comment-head">
                  <Author author={c.author} />
                  <Time ms={Number(c.createdAtMs)} />
                  {c === head && lines(c) && <span className="muted">on {lines(c)}</span>}
                  {resolved && c === head && <span className="pill">Resolved</span>}
                </div>
                <Markdown text={c.body.replace(SUGGESTIONS, "").trim()} className="comment-body markdown" />
                {c.suggestion && <Suggestion text={c.suggestion.replace(/\n+$/, "")} original={c === head ? original : undefined} />}
              </div>
            ))}
            {replying ? (
              <div className="thread-reply">
                <Composer
                  label="Reply"
                  submitLabel="Reply"
                  autoFocus
                  pending={add.isPending}
                  error={add.error ? (add.error as Error).message : null}
                  onSubmit={submit}
                  onCancel={() => setReplying(false)}
                />
              </div>
            ) : (
              <div className="thread-foot">
                <button type="button" className="reply-field" onClick={() => setReplying(true)}>
                  Reply…
                </button>
                <button type="button" className="button button-quiet" onClick={() => resolve.mutate({ thread: head.thread, resolved: !resolved })}>
                  {resolved ? "Reopen" : "Resolve"}
                </button>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
