import { useNavigate } from "@tanstack/react-router";
import { Command } from "cmdk";
import { useDeferredValue, useState } from "react";
import { Dialog, Modal, ModalOverlay } from "react-aria-components";
import { useFind, useTip } from "../data";
import { useInbox, useRepositories } from "../platform-data";
import { useRepoMaybe } from "../repo";
import { Author, Icon, type IconName } from "../ui";
import { useRefName, type Theme } from "./hooks";

export type MenuMode = "all" | "files";

interface Props {
  open: MenuMode | null;
  onClose(): void;
  theme: Theme;
  setTheme(t: Theme): void;
  showShortcuts(): void;
}

/** One place to reach anything: files, repositories, reviews waiting on you, and actions. */
export function CommandMenu({ open, onClose, theme, setTheme, showShortcuts }: Props) {
  const navigate = useNavigate();
  const repo = useRepoMaybe();
  const [query, setQuery] = useState("");
  const deferred = useDeferredValue(query.trim());
  const repos = useRepositories();
  const inbox = useInbox();
  const filesOnly = open === "files";

  const run = (f: () => void) => {
    onClose();
    setQuery("");
    f();
  };
  const nextTheme: Theme = theme === "system" ? "dark" : theme === "dark" ? "light" : "system";
  const rp = repo ? { owner: repo.owner, repo: repo.name } : null;
  const go = (label: string, icon: IconName, f: () => void, hint?: string) => (
    <Command.Item key={label} value={`go ${label}`} onSelect={() => run(f)}>
      <Icon name={icon} /> {label}
      {hint && <span className="hint">{hint}</span>}
    </Command.Item>
  );

  return (
    <ModalOverlay className="overlay" isDismissable isOpen={open !== null} onOpenChange={(o) => !o && run(() => {})}>
      <Modal className="palette">
        <Dialog aria-label={filesOnly ? "Go to file" : "Command menu"} style={{ outline: "none" }}>
          <Command label={filesOnly ? "Go to file" : "Command menu"} loop>
            <Command.Input
              autoFocus
              value={query}
              onValueChange={setQuery}
              placeholder={filesOnly && repo ? `Go to a file in ${repo.name}…` : "Search files, repositories, reviews and actions…"}
            />
            <Command.List>
              <Command.Empty>{deferred ? `Nothing matches “${deferred}”.` : "Type to search."}</Command.Empty>
              {repo && open && <Files query={deferred} onPick={(p) => run(() => navigate({ to: "/$owner/$repo/code/$", params: { ...rp!, _splat: p } }))} />}

              {!filesOnly && (
                <>
                  {inbox.data && inbox.data.length > 0 && (
                    <Command.Group heading="Waiting on you">
                      {inbox.data.slice(0, deferred ? 50 : 4).map((i) => {
                        const cr = i.changeRequest!;
                        return (
                          <Command.Item
                            key={`${cr.repository?.name}#${cr.number}`}
                            value={`review ${cr.repository?.name} ${cr.number} ${cr.title} ${cr.author}`}
                            onSelect={() => run(() => navigate({ to: "/$owner/$repo/reviews/$number", params: { owner: cr.repository!.owner, repo: cr.repository!.name, number: String(cr.number) } }))}
                          >
                            <Author author={cr.author} compact />
                            <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{cr.title}</span>
                            <span className="hint">
                              {cr.repository?.name} #{String(cr.number)}
                            </span>
                          </Command.Item>
                        );
                      })}
                    </Command.Group>
                  )}

                  {rp && (
                    <Command.Group heading={`In ${repo!.name}`}>
                      {go("Overview", "repo", () => navigate({ to: "/$owner/$repo", params: rp }))}
                      {go("Code", "code", () => navigate({ to: "/$owner/$repo/code/$", params: { ...rp, _splat: "" } }), "g c")}
                      {go("Changes", "changes", () => navigate({ to: "/$owner/$repo/changes", params: rp }), "g l")}
                      {go("Reviews", "review", () => navigate({ to: "/$owner/$repo/reviews", params: rp }), "g r")}
                      {go("Locks", "lock", () => navigate({ to: "/$owner/$repo/locks", params: rp }), "g k")}
                      {go("Agents", "bot", () => navigate({ to: "/$owner/$repo/agents", params: rp }))}
                      {go("Settings", "settings", () => navigate({ to: "/$owner/$repo/settings", params: rp }))}
                    </Command.Group>
                  )}

                  {repos.data && (
                    <Command.Group heading="Repositories">
                      {repos.data.map((r) => (
                        <Command.Item key={r.name} value={`repository ${r.owner}/${r.name} ${r.description}`} onSelect={() => run(() => navigate({ to: "/$owner/$repo", params: { owner: r.owner, repo: r.name } }))}>
                          <Icon name="repo" />
                          <span>
                            <span className="muted">{r.owner}/</span>
                            {r.name}
                          </span>
                          <span className="hint">{r.lands24h} lands today</span>
                        </Command.Item>
                      ))}
                    </Command.Group>
                  )}

                  <Command.Group heading="Go to">
                    {go("Home", "home", () => navigate({ to: "/" }), "g h")}
                    {go("Inbox", "inbox", () => navigate({ to: "/inbox" }), "g i")}
                    {go("Tokens", "key", () => navigate({ to: "/settings/tokens" }))}
                    {go("Connection", "plug", () => navigate({ to: "/connect" }))}
                  </Command.Group>

                  <Command.Group heading="Actions">
                    <Command.Item value="theme appearance dark light system" onSelect={() => run(() => setTheme(nextTheme))}>
                      <Icon name={nextTheme === "dark" ? "moon" : "sun"} /> Use {nextTheme === "system" ? "the system theme" : `the ${nextTheme} theme`}
                    </Command.Item>
                    <Command.Item value="keyboard shortcuts help" onSelect={() => run(showShortcuts)}>
                      <Icon name="keyboard" /> Keyboard shortcuts <span className="hint">?</span>
                    </Command.Item>
                  </Command.Group>
                </>
              )}
            </Command.List>
            <div className="palette-foot" aria-hidden="true">
              <span>
                <kbd>↑</kbd> <kbd>↓</kbd> move
              </span>
              <span>
                <kbd>↵</kbd> open
              </span>
              <span>
                <kbd>esc</kbd> close
              </span>
            </div>
          </Command>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}

/** Files in the repository in view whose path contains the query. */
function Files({ query, onPick }: { query: string; onPick(path: string): void }) {
  const ref = useRefName();
  const tip = useTip(ref);
  const files = useFind(tip.data ?? undefined, query);
  if (!query || !files.data?.paths.length) return null;
  return (
    <Command.Group heading={files.data.truncated ? "Files (first 50)" : "Files"}>
      {files.data.paths.map((p) => (
        <Command.Item key={p} value={`file ${p}`} onSelect={() => onPick(p)}>
          <Icon name="file" />
          <Path path={p} query={query} />
        </Command.Item>
      ))}
    </Command.Group>
  );
}

function Path({ path, query }: { path: string; query: string }) {
  const at = path.indexOf(query);
  if (at < 0) return <span className="mono">{path}</span>;
  return (
    <span className="mono" style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
      <span className="muted">{path.slice(0, at)}</span>
      <span className="match">{path.slice(at, at + query.length)}</span>
      {path.slice(at + query.length)}
    </span>
  );
}
