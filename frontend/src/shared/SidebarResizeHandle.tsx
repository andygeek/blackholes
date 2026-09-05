import { useEffect, useRef, useState } from "react";
import { postNative } from "./native";

const MIN_WIDTH = 220;
const MAX_WIDTH = 420;
const clamp = (width: number) => Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, width));

/** Pointer capture keeps dragging reliable across embedded WebView boundaries. */
export function SidebarResizeHandle({ width, label, left, right = 0, hitWidth = 12, edge = "right", fixed = false }: {
  width: number; label: string; left?: number; right?: number; hitWidth?: number; edge?: "left" | "right" | "center"; fixed?: boolean;
}) {
  const drag = useRef<{ x: number; width: number; next: number } | null>(null);
  const frame = useRef<number | null>(null);
  const [active, setActive] = useState(false);
  const finish = () => {
    if (frame.current !== null) cancelAnimationFrame(frame.current);
    frame.current = null;
    const current = drag.current;
    drag.current = null;
    if (current) postNative({ type: "set_sidebar_width", width: current.next, commit: true });
    setActive(false);
  };
  useEffect(() => {
    window.addEventListener("blur", finish);
    return () => {
      window.removeEventListener("blur", finish);
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      if (drag.current) postNative({ type: "set_sidebar_width", width: drag.current.next, commit: true });
      drag.current = null;
    };
  }, []);
  return <div
    className={`sidebar-resize-handle is-${edge}-edge` + (active ? " is-dragging" : "")}
    style={{ ...(left === undefined ? { right } : { left }), width: hitWidth, ...(fixed ? { position: "fixed" } : {}) }}
    role="separator" tabIndex={0} aria-orientation="vertical" aria-label={label}
    aria-valuemin={MIN_WIDTH} aria-valuemax={MAX_WIDTH} aria-valuenow={Math.round(width)}
    onPointerDown={(event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      event.stopPropagation();
      event.currentTarget.setPointerCapture(event.pointerId);
      drag.current = { x: event.screenX, width, next: width };
      setActive(true);
    }}
    onPointerMove={(event) => {
      if (!drag.current) return;
      drag.current.next = clamp(drag.current.width + event.screenX - drag.current.x);
      if (frame.current !== null) return;
      frame.current = requestAnimationFrame(() => {
        frame.current = null;
        if (drag.current) postNative({ type: "set_sidebar_width", width: drag.current.next, commit: false });
      });
    }}
    onPointerUp={finish} onPointerCancel={finish} onLostPointerCapture={finish}
    onKeyDown={(event) => {
      const step = event.shiftKey ? 1 : 10;
      const next = event.key === "ArrowLeft" ? width - step : event.key === "ArrowRight" ? width + step
        : event.key === "Home" ? MIN_WIDTH : event.key === "End" ? MAX_WIDTH : null;
      if (next === null) return;
      event.preventDefault();
      event.stopPropagation();
      postNative({ type: "set_sidebar_width", width: clamp(next), commit: true });
    }}
  />;
}
