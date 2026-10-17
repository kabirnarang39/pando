import { useNavigate } from "@tanstack/react-router";
import { Command } from "cmdk";
import { useDeferredValue, useState } from "react";
import { Dialog, Modal, ModalOverlay } from "react-aria-components";
import { title, type Change } from "../api/client";
import { useFind, useRefs } from "../data";
import { Author, Icon, Time } from "../ui";
import type { Theme } from "./hooks";

export type MenuMode = "all" | "files";

interface Props {
  open: MenuMode | null;
  onClose(): void;
  tip: string | null | undefined;
  refName: string;
  recent: Change[] | undefined;
  theme: Theme;
  setTheme(t: Theme): void;
  showShortcuts(): void;
}

/** One place to reach anything: files, changes, branches and actions. */
export function CommandMenu({ open, onClose, tip, refName, recent, theme, setTheme, showShortcuts }: Props) {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const deferred = useDeferredValue(query.trim());
  const files = useFind(open ? (tip ?? undefined) : undefined, deferred);
  const refs = useRefs();
  const filesOnly = open === "files";

  const run = (f: () => void) => {
    onClose();
    setQuery("");
    f();
  };
  const go = (to: "/changes" | "/locks" | "/connect") => run(() => navigate({ to, search: (s) => ({ ref: s.ref }) }));
  const nextTheme: Theme = theme === "system" ? "dark" : theme === "dark" ? "light" : "system";

  return (
    <ModalOverlay className="overlay" isDismissable isOpen={open !== null} onOpenChange={(o) => !o && run(() => {})}>
      <Modal className="palette">
        <Dialog aria-label={filesOnly ? "Go to file" : "Command menu"} style={{ outline: "none" }}>
          <Command label={filesOnly ? "Go to file" : "Command menu"} loop>
            <Command.Input
              autoFocus
              value={query}
              onValueChange={setQuery}
              placeholder={filesOnly ? `Go to a file on ${refName}…` : "Search files, changes, branches and actions…"}
            />
            <Command.List>
              <Command.Empty>
                {deferred && files.isFetching ? "Searching…" : deferred ? `Nothing matches “${deferred}”.` : "Type to search files."}
              </Command.Empty>

              {deferred && files.data && files.data.paths.length > 0 && (
                <Command.Group heading={files.data.truncated ? "Files (first 50)" : "Files"}>
                  {files.data.paths.map((p) => (
                    <Command.Item
                      key={p}
                      value={`file ${p}`}
                      onSelect={() => run(() => navigate({ to: "/code/$", params: { _splat: p }, search: { ref: refName === "main" ? undefined : refName } }))}
                    >
                      <Icon name="file" />
                      <Path path={p} query={deferred} />
                    </Command.Item>
                  ))}
                </Command.Group>
              )}

              {!filesOnly && (
                <>
                  <Command.Group heading="Go to">
                    <Command.Item value="go changes landed feed" onSelect={() => go("/changes")}>
                      <Icon name="changes" /> Changes <span className="hint">g then l</span>
                    </Command.Item>
                    <Command.Item
                      value="go code browse files tree"
                      onSelect={() => run(() => navigate({ to: "/code/$", params: { _splat: "" }, search: (s) => ({ ref: s.ref }) }))}
                    >
                      <Icon name="code" /> Code <span className="hint">g then c</span>
                    </Command.Item>
                    <Command.Item value="go locks locked files" onSelect={() => go("/locks")}>
                      <Icon name="lock" /> Locks <span className="hint">g then k</span>
                    </Command.Item>
                    <Command.Item value="go connection server token settings" onSelect={() => go("/connect")}>
                      <Icon name="plug" /> Connection
                    </Command.Item>
                  </Command.Group>

                  {recent && recent.length > 0 && (
                    <Command.Group heading={`Recent changes on ${refName}`}>
                      {recent.slice(0, deferred ? 100 : 6).map((c) => (
                        <Command.Item
                          key={c.id}
                          value={`change ${c.id} ${c.author} ${c.message}`}
                          onSelect={() => run(() => navigate({ to: "/changes", search: (s) => ({ ref: s.ref, c: c.id }) }))}
                        >
                          <Author author={c.author} compact />
                          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{title(c.message)}</span>
                          <span className="hint">
                            <Time ms={c.time} />
                          </span>
                        </Command.Item>
                      ))}
                    </Command.Group>
                  )}

                  {refs.data && refs.data.length > 1 && (
                    <Command.Group heading="Switch branch">
                      {refs.data.map((r) => (
                        <Command.Item
                          key={r.name}
                          value={`branch ref switch ${r.name}`}
                          onSelect={() => run(() => navigate({ to: ".", search: (s) => ({ ...s, ref: r.name === "main" ? undefined : r.name }) }))}
                        >
                          <Icon name="branch" />
                          <span className="mono">{r.name}</span>
                          {r.name === refName && <span className="hint">current</span>}
                        </Command.Item>
                      ))}
                    </Command.Group>
                  )}

                  <Command.Group heading="Actions">
                    <Command.Item value="theme appearance dark light system" onSelect={() => run(() => setTheme(nextTheme))}>
                      <Icon name={nextTheme === "dark" ? "moon" : "sun"} /> Use {nextTheme === "system" ? "the system theme" : `${nextTheme} theme`}
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

/** A path with the matched part emphasised, directory dimmed. */
function Path({ path, query }: { path: string; query: string }) {
  const at = path.indexOf(query);
  const slash = path.lastIndexOf("/");
  if (at < 0) return <span className="mono">{path}</span>;
  return (
    <span className="mono" style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
      <span className={at > slash ? "muted" : undefined}>{path.slice(0, at)}</span>
      <span className="match">{path.slice(at, at + query.length)}</span>
      {path.slice(at + query.length)}
    </span>
  );
}
