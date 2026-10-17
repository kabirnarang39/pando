import { useSearch } from "@tanstack/react-router";
import { useEffect, useState } from "react";

/** The branch every view is looking at. */
export function useRefName(): string {
  const s = useSearch({ strict: false }) as { ref?: string };
  return s.ref ?? "main";
}

export type Theme = "system" | "light" | "dark";
const KEY = "pando.console.theme";

function stored(): Theme {
  try {
    const v = localStorage.getItem(KEY);
    return v === "light" || v === "dark" ? v : "system";
  } catch {
    return "system";
  }
}

export function useTheme(): [Theme, (t: Theme) => void] {
  const [theme, setTheme] = useState<Theme>(stored);
  useEffect(() => {
    if (theme === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem(KEY, theme);
    } catch {
      // Not remembered across visits without storage; it still applies now.
    }
  }, [theme]);
  return [theme, setTheme];
}

/** Whether focus is in a place where typing means text, not a shortcut. */
export function typing(e: KeyboardEvent): boolean {
  const t = e.target as HTMLElement | null;
  return !!t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) || !!t.closest("[role=dialog]"));
}

export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
export const modKey = isMac ? "⌘" : "Ctrl";
