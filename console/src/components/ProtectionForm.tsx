import { useState } from "react";
import type { Protection } from "../gen/pando_platform_pb";
import { useSetProtection } from "../platform-data";

const list = (s: string) => s.split(/[\s,]+/).filter(Boolean);

/** Protect a branch, or change how one is protected. */
export function ProtectionForm({ repo, existing, onDone }: { repo: string; existing?: Protection; onDone(): void }) {
  const save = useSetProtection(repo);
  const [pattern, setPattern] = useState(existing?.pattern ?? "main");
  const [approvals, setApprovals] = useState(existing?.requiredApprovals ?? 1);
  const [checks, setChecks] = useState(existing?.requiredChecks.join(", ") ?? "");
  const [dismissStale, setDismissStale] = useState(existing?.dismissStale ?? true);
  const [agents, setAgents] = useState(existing?.agentsLandDirectly ?? false);
  const [bypass, setBypass] = useState(existing?.bypass.join(", ") ?? "admins");
  return (
    <form
      className="protection-form"
      aria-label={existing ? `Protection of ${existing.pattern}` : "Protect a branch"}
      onSubmit={(e) => {
        e.preventDefault();
        save.mutate(
          { pattern: pattern.trim(), requiredApprovals: approvals, requiredChecks: list(checks), dismissStale, agentsLandDirectly: agents, bypass: list(bypass) },
          { onSuccess: onDone },
        );
      }}
    >
      <div className="token-form-row">
        <label>
          <span>Branch</span>
          <input className="input" value={pattern} onChange={(e) => setPattern(e.target.value)} disabled={!!existing} placeholder="main or release/*" spellCheck={false} required />
        </label>
        <label>
          <span>Approvals</span>
          <input className="input" type="number" min={0} max={10} value={approvals} onChange={(e) => setApprovals(Math.max(0, Math.min(10, Number(e.target.value))))} />
        </label>
      </div>
      <label>
        <span>Checks that must pass</span>
        <input className="input" value={checks} onChange={(e) => setChecks(e.target.value)} placeholder="build, test" spellCheck={false} />
      </label>
      <label>
        <span>May land without review</span>
        <input className="input" value={bypass} onChange={(e) => setBypass(e.target.value)} placeholder="admins, lumen/release-managers" spellCheck={false} />
      </label>
      <label className="check">
        <input type="checkbox" checked={dismissStale} onChange={(e) => setDismissStale(e.target.checked)} /> A new version needs new approvals
      </label>
      <label className="check">
        <input type="checkbox" checked={agents} onChange={(e) => setAgents(e.target.checked)} /> Agents land directly, without review
      </label>
      {save.error && (
        <p className="status" data-kind="error" role="alert">
          {(save.error as Error).message}
        </p>
      )}
      <div className="token-form-actions">
        <button type="button" className="button button-quiet" onClick={onDone}>
          Cancel
        </button>
        <button type="submit" className="button" data-primary="" disabled={!pattern.trim() || save.isPending}>
          {save.isPending ? "Saving…" : existing ? "Save" : "Protect"}
        </button>
      </div>
    </form>
  );
}
