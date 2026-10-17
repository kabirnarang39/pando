import { useState } from "react";
import { Role } from "../gen/pando_platform_pb";

export const roleNames: Record<number, string> = {
  [Role.READ]: "Read",
  [Role.TRIAGE]: "Triage",
  [Role.WRITE]: "Write",
  [Role.MAINTAIN]: "Maintain",
  [Role.ADMIN]: "Admin",
};

const choices = [Role.READ, Role.TRIAGE, Role.WRITE, Role.MAINTAIN, Role.ADMIN];

/** A role picker; `onChange` fires with the new role. */
export function RoleSelect({ value, onChange, label, disabled }: { value: Role; onChange(r: Role): void; label: string; disabled?: boolean }) {
  return (
    <select className="input role-select" value={value} aria-label={label} disabled={disabled} onChange={(e) => onChange(Number(e.target.value) as Role)}>
      {choices.map((r) => (
        <option key={r} value={r}>
          {roleNames[r]}
        </option>
      ))}
    </select>
  );
}

/** "Add someone with a role": a login (or team) and a role, one row. */
export function AddPrincipal({
  label,
  placeholder,
  initial = Role.WRITE,
  pending,
  error,
  onAdd,
}: {
  label: string;
  placeholder: string;
  initial?: Role;
  pending?: boolean;
  error?: string | null;
  onAdd(principal: string, role: Role, done: () => void): void;
}) {
  const [who, setWho] = useState("");
  const [role, setRole] = useState(initial);
  return (
    <form
      className="add-principal"
      onSubmit={(e) => {
        e.preventDefault();
        if (who.trim()) onAdd(who.trim(), role, () => setWho(""));
      }}
    >
      <input className="input" value={who} onChange={(e) => setWho(e.target.value)} placeholder={placeholder} aria-label={label} autoComplete="off" spellCheck={false} />
      <RoleSelect value={role} onChange={setRole} label="Role" />
      <button type="submit" className="button" data-primary="" disabled={!who.trim() || pending}>
        {pending ? "Adding…" : "Add"}
      </button>
      {error && (
        <p className="status" data-kind="error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}
