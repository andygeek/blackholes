import type { ReactNode } from "react";

export type NoticeKind = "completed" | "input-required";

/** Keep the status attached to its icon without shifting the row's columns. */
export function NavigationStatusIcon({ children, notice, working = false, large = false }: {
  children: ReactNode;
  notice: NoticeKind | null;
  working?: boolean;
  large?: boolean;
}) {
  return <span aria-hidden="true" className={`${large ? "terminal-agent-avatar" : "row-icon"} navigation-status-icon${notice ? " has-notice" : working ? " is-working" : ""}${notice === "input-required" ? " is-awaiting-input" : ""}`}>
    <span className="navigation-status-artwork">{children}</span>
    {notice ? <span className="navigation-status-dot" /> : working ? <span className="navigation-status-spinner" /> : null}
  </span>;
}
