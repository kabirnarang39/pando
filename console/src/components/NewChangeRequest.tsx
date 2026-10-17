import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { useConnection } from "../connection";
import { useRefs } from "../data";
import { useCreateChangeRequest, useRepository } from "../platform-data";
import { useRepo, useRepoParams } from "../repo";

/** Propose a branch for landing on another. */
export function NewChangeRequest({ onCancel }: { onCancel(): void }) {
  const { key } = useRepo();
  const rp = useRepoParams();
  const navigate = useNavigate();
  const { target: conn } = useConnection();
  const refs = useRefs();
  const repo = useRepository(key);
  const create = useCreateChangeRequest(key);
  const main = repo.data?.defaultBranch ?? "main";
  const branches = (refs.data ?? []).map((r) => r.name).filter((n) => !n.startsWith("_"));
  const [target, setTarget] = useState(main);
  const [source, setSource] = useState("");
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [reviewers, setReviewers] = useState("");
  const [draft, setDraft] = useState(false);
  const from = source || branches.find((b) => b !== target) || "";
  return (
    <form
      className="panel token-form"
      aria-label="New change request"
      onSubmit={(e) => {
        e.preventDefault();
        create.mutate(
          { source: from, target, title: title.trim(), body, draft, reviewers: reviewers.split(/[\s,]+/).filter(Boolean) },
          { onSuccess: (cr) => navigate({ to: "/$owner/$repo/reviews/$number", params: { ...rp, number: String(cr.number) } }) },
        );
      }}
    >
      {conn.kind === "demo" && <p className="status" data-kind="info">In the demo, agents open change requests as they work. Connect your server to open your own.</p>}
      <div className="token-form-row">
        <label>
          <span>Land</span>
          <select className="input" value={from} onChange={(e) => setSource(e.target.value)} aria-label="Branch to land">
            {branches
              .filter((b) => b !== target)
              .map((b) => (
                <option key={b}>{b}</option>
              ))}
          </select>
        </label>
        <label>
          <span>into</span>
          <select className="input" value={target} onChange={(e) => setTarget(e.target.value)} aria-label="Target branch">
            {[main, ...branches.filter((b) => b !== main)].map((b) => (
              <option key={b}>{b}</option>
            ))}
          </select>
        </label>
      </div>
      <label>
        <span>Title</span>
        <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="What it changes, in a sentence" required maxLength={300} autoFocus />
      </label>
      <label>
        <span>Description</span>
        <textarea className="input composer-text" value={body} onChange={(e) => setBody(e.target.value)} placeholder="Why, and how to check it. Markdown works." rows={4} />
      </label>
      <label>
        <span>Reviewers</span>
        <input className="input" value={reviewers} onChange={(e) => setReviewers(e.target.value)} placeholder="mira, dev" spellCheck={false} />
      </label>
      <label className="check" style={{ display: "flex", gap: 8, alignItems: "center", fontWeight: 400 }}>
        <input type="checkbox" checked={draft} onChange={(e) => setDraft(e.target.checked)} /> Draft: not ready for review yet
      </label>
      {create.error && (
        <p className="status" data-kind="error" role="alert">
          {(create.error as Error).message}
        </p>
      )}
      <div className="token-form-actions">
        <button type="button" className="button button-quiet" onClick={onCancel}>
          Cancel
        </button>
        <button type="submit" className="button" data-primary="" disabled={conn.kind === "demo" || !title.trim() || !from || create.isPending}>
          {create.isPending ? "Opening…" : "Open change request"}
        </button>
      </div>
    </form>
  );
}
