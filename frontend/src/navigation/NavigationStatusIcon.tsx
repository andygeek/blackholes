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
    {notice ? <span key="notice" className="navigation-status-dot" /> : working ? <svg key="working" className="navigation-status-spinner" viewBox="0 0 24 24" fill="none">
      <circle cx="12" cy="12" r="9" stroke="currentColor" strokeOpacity=".2" strokeWidth="2" />
      <path d="M12 3a9 9 0 0 1 9 9" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
        {/* Keep progress running independently of global CSS transition rules. */}
        <animateTransform attributeName="transform" type="rotate" from="0 12 12" to="360 12 12" dur=".9s" repeatCount="indefinite" />
      </path>
    </svg> : null}
  </span>;
}
