import { useNavigate } from "@tanstack/react-router";
import { explain } from "../api/client";
import { useState } from "react";
import { Visibility } from "../gen/pando_platform_pb";
import { useCreateRepository } from "../platform-data";

/** Create a repository in an organisation, then open it (it says how to put code in it). */
export function NewRepository({ org, onCancel }: { org: string; onCancel(): void }) {
  const create = useCreateRepository();
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [visibility, setVisibility] = useState(Visibility.PRIVATE);
  return (
    <form
      className="token-form"
      aria-label="New repository"
      onSubmit={(e) => {
        e.preventDefault();
        const n = name.trim();
        create.mutate({ owner: org, name: n, description: description.trim(), visibility }, { onSuccess: () => navigate({ to: "/$owner/$repo", params: { owner: org, repo: n } }) });
      }}
    >
      <label>
        <span>Name</span>
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="web" required autoFocus autoComplete="off" spellCheck={false} maxLength={100} />
      </label>
      <label>
        <span>Description (optional)</span>
        <input className="input" value={description} onChange={(e) => setDescription(e.target.value)} placeholder="What lives here" />
      </label>
      <label>
        <span>Who can see it</span>
        <select className="input" value={visibility} onChange={(e) => setVisibility(Number(e.target.value))}>
          <option value={Visibility.PRIVATE}>Private: members you give access to</option>
          <option value={Visibility.INTERNAL}>Internal: everyone in {org}</option>
        </select>
      </label>
      {create.error && (
        <p className="status" data-kind="error" role="alert">
          {explain(create.error)}
        </p>
      )}
      <div className="token-form-actions">
        <button type="button" className="button button-quiet" onClick={onCancel}>
          Cancel
        </button>
        <button type="submit" className="button" data-primary="" disabled={!name.trim() || create.isPending}>
          {create.isPending ? "Creating…" : "Create repository"}
        </button>
      </div>
    </form>
  );
}
