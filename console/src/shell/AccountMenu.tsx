import { useNavigate } from "@tanstack/react-router";
import { useEffect, useRef, type ReactNode } from "react";
import { Button, Dialog, Menu, MenuItem, MenuTrigger, Modal, ModalOverlay, Popover, Separator } from "react-aria-components";
import { Icon } from "../ui";
import { modKey } from "./hooks";

/** The account menu in the sidebar's foot; `children` is the button's face. */
export function AccountMenu({ children, showShortcuts, open }: { children: ReactNode; showShortcuts(): void; open?: boolean }) {
  const navigate = useNavigate();
  // Clicked before this loaded: open now, from a focused trigger, so focus returns to it on close.
  const button = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (open) (button.current?.focus(), button.current?.click());
  }, [open]);
  return (
    <MenuTrigger>
      <Button ref={button} className="me" aria-label="Account and settings">
        {children}
      </Button>
      <Popover placement="top start" offset={6}>
        <Menu
          className="menu"
          onAction={(k) => {
            if (k === "tokens") navigate({ to: "/settings/tokens" });
            else if (k === "connect") navigate({ to: "/connect" });
            else if (k === "keys") showShortcuts();
          }}
        >
          <MenuItem id="tokens" className="menu-item">
            <Icon name="key" /> Tokens
          </MenuItem>
          <MenuItem id="connect" className="menu-item">
            <Icon name="plug" /> Connection
          </MenuItem>
          <Separator className="menu-sep" />
          <MenuItem id="keys" className="menu-item">
            <Icon name="keyboard" /> Keyboard shortcuts
          </MenuItem>
        </Menu>
      </Popover>
    </MenuTrigger>
  );
}

const shortcuts: [string[], string][] = [
  [[modKey, "K"], "Search or jump anywhere"],
  [["/"], "Search"],
  [["t"], "Go to a file in this repository"],
  [["g", "h"], "Home"],
  [["g", "i"], "Inbox"],
  [["g", "c"], "Code"],
  [["g", "l"], "Changes"],
  [["g", "r"], "Reviews"],
  [["g", "k"], "Locks"],
  [["j", "k"], "Next and previous item"],
  [["n", "p"], "Next and previous file in a review"],
  [["a"], "Approve or review the change request"],
  [["b"], "Show or hide blame"],
  [["h"], "Show a file's history"],
  [["u"], "Unified or split diff"],
  [["?"], "This list"],
];

/** Every keyboard shortcut, in a dialog. */
export function Shortcuts({ open, onClose }: { open: boolean; onClose(): void }) {
  return (
    <ModalOverlay className="overlay" isDismissable isOpen={open} onOpenChange={(o) => !o && onClose()}>
      <Modal>
        <Dialog className="dialog" aria-labelledby="keys-title">
          <h2 id="keys-title">Keyboard shortcuts</h2>
          <dl className="keys">
            {shortcuts.map(([k, what]) => (
              <div key={what} style={{ display: "contents" }}>
                <dt>
                  {k.map((x) => (
                    <kbd key={x}>{x}</kbd>
                  ))}
                </dt>
                <dd>{what}</dd>
              </div>
            ))}
          </dl>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}
