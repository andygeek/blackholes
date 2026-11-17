import type { ReactNode } from "react";

export type NoticeKind = "completed" | "input-required";

/** Outline of a gear around a round hole, to fill with the evenodd rule. */
function gearPath(x: number, y: number, teeth: number, outer: number, inner: number, hole: number) {
  const at = (radius: number, angle: number) => `${(x + radius * Math.cos(angle)).toFixed(2)} ${(y + radius * Math.sin(angle)).toFixed(2)}`;
  const step = (Math.PI * 2) / teeth;
  let outline = "";
  for (let tooth = 0; tooth < teeth; tooth++) {
    const angle = tooth * step;
    outline += `${tooth ? "L" : "M"}${at(inner, angle)}L${at(outer, angle + step * .14)}L${at(outer, angle + step * .36)}L${at(inner, angle + step * .5)}`;
  }
  return `${outline}ZM${x + hole} ${y}a${hole} ${hole} 0 1 0 ${-2 * hole} 0a${hole} ${hole} 0 1 0 ${2 * hole} 0Z`;
}

// Two meshed gears on the 24-unit grid the spinner uses.
const LARGE_GEAR = gearPath(9.2, 14.6, 8, 7.2, 5.4, 2.3);
const SMALL_GEAR = gearPath(17.6, 6.8, 6, 4.9, 3.5, 1.5);

/** Keep the status attached to its icon without shifting the row's columns. */
export function NavigationStatusIcon({ children, notice, working = false, background = false, backgroundInside = false, large = false }: {
  children: ReactNode;
  notice: NoticeKind | null;
  working?: boolean;
  /** Something keeps running behind this session. A notice or the spinner wins. */
  background?: boolean;
  /** A session inside this project or group has something running behind it. */
  backgroundInside?: boolean;
  large?: boolean;
}) {
  const status = notice ? " has-notice" : working ? " is-working" : background ? " has-background" : backgroundInside ? " has-background-inside" : "";
  return <span aria-hidden="true" className={`${large ? "terminal-agent-avatar" : "row-icon"} navigation-status-icon${status}${notice === "input-required" ? " is-awaiting-input" : ""}`}>
    <span className="navigation-status-artwork">{children}</span>
    {notice ? <span key="notice" className="navigation-status-dot" /> : working ? <svg key="working" className="navigation-status-spinner" viewBox="0 0 24 24" fill="none">
      <circle cx="12" cy="12" r="9" stroke="currentColor" strokeOpacity=".2" strokeWidth="2" />
      <path d="M12 3a9 9 0 0 1 9 9" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
        {/* Keep progress running independently of global CSS transition rules. */}
        <animateTransform attributeName="transform" type="rotate" from="0 12 12" to="360 12 12" dur=".9s" repeatCount="indefinite" />
      </path>
    </svg> : background ? <svg key="background" className="navigation-status-gears" viewBox="0 0 24 24" fill="currentColor" fillRule="evenodd">
      {/* Opposite turns at the ratio of their teeth, so the gears read as meshed. */}
      <path d={LARGE_GEAR}>
        <animateTransform attributeName="transform" type="rotate" from="0 9.2 14.6" to="360 9.2 14.6" dur="5s" repeatCount="indefinite" />
      </path>
      <path d={SMALL_GEAR}>
        <animateTransform attributeName="transform" type="rotate" from="360 17.6 6.8" to="0 17.6 6.8" dur="3.75s" repeatCount="indefinite" />
      </path>
    </svg> : backgroundInside ? <span key="background" className="navigation-status-dot" /> : null}
  </span>;
}
