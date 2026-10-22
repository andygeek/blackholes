import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";

const MIN_FRACTION = .1;
const MAX_FRACTION = .9;
const DEFAULT_FRACTION = .4;
const MIN_SECTION_HEIGHT = 64;

interface Drag {
  pointer: number;
  element: HTMLDivElement;
  y: number;
  fraction: number;
  height: number;
  next: number;
}

/** A captured horizontal separator adjusts the two independent scroll regions. */
export function ResizableSidebarSections({ children, fraction, agentsCollapsed, projectsCollapsed, disabled, label, onResize }: {
  children: ReactNode;
  fraction: number;
  agentsCollapsed: boolean;
  projectsCollapsed: boolean;
  disabled: boolean;
  label: string;
  onResize(fraction: number, commit: boolean): void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const frame = useRef<number | null>(null);
  const resize = useRef(onResize);
  resize.current = onResize;
  const [localFraction, setLocalFraction] = useState<number | null>(null);
  const [active, setActive] = useState(false);
  const displayed = localFraction ?? fraction;
  const hidden = agentsCollapsed || projectsCollapsed;

  const clamp = (value: number, height = container.current?.clientHeight || 1) => {
    const minimum = Math.min(.5, Math.max(MIN_FRACTION, MIN_SECTION_HEIGHT / height));
    return Math.max(minimum, Math.min(1 - minimum, value));
  };

  const finish = () => {
    if (frame.current !== null) cancelAnimationFrame(frame.current);
    frame.current = null;
    const current = drag.current;
    drag.current = null;
    if (!current) return;
    setLocalFraction(current.next);
    setActive(false);
    resize.current(current.next, true);
    if (current.element.hasPointerCapture(current.pointer)) current.element.releasePointerCapture(current.pointer);
  };

  useEffect(() => {
    if (disabled || hidden) finish();
  }, [disabled, hidden]);

  useEffect(() => {
    if (!drag.current && localFraction !== null && Math.abs(fraction - localFraction) < .0001) setLocalFraction(null);
  }, [fraction, localFraction]);

  useEffect(() => {
    window.addEventListener("blur", finish);
    return () => {
      window.removeEventListener("blur", finish);
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      if (drag.current) resize.current(drag.current.next, true);
      drag.current = null;
    };
  }, []);

  const commit = (value: number) => {
    const next = clamp(value);
    setLocalFraction(next);
    resize.current(next, true);
  };

  return <div ref={container} className="sidebar-sections" inert={disabled}
    data-agents-collapsed={agentsCollapsed} data-projects-collapsed={projectsCollapsed}
    style={{ "--sidebar-agents-share": `${displayed}fr`, "--sidebar-projects-share": `${1 - displayed}fr` } as CSSProperties}>
    {children}
    <div className={`sidebar-section-resize-handle${active ? " is-dragging" : ""}`} hidden={hidden}
      role="separator" tabIndex={disabled || hidden ? -1 : 0} aria-orientation="horizontal"
      aria-label={label} aria-controls="sidebar-agents-content sidebar-projects-content"
      aria-valuemin={MIN_FRACTION * 100} aria-valuemax={MAX_FRACTION * 100} aria-valuenow={Math.round(displayed * 100)}
      title={label}
      onPointerDown={event => {
        if (disabled || hidden || event.button !== 0 || !event.isPrimary) return;
        const height = container.current?.clientHeight || 0;
        const agents = container.current?.querySelector<HTMLElement>(".agents-shell");
        if (!height || !agents) return;
        event.preventDefault();
        event.stopPropagation();
        event.currentTarget.setPointerCapture(event.pointerId);
        const current = agents.getBoundingClientRect().height / height;
        drag.current = { pointer: event.pointerId, element: event.currentTarget, y: event.screenY, fraction: current, height, next: current };
        setActive(true);
      }}
      onPointerMove={event => {
        const current = drag.current;
        if (!current || current.pointer !== event.pointerId) return;
        if ((event.buttons & 1) === 0) { finish(); return; }
        current.next = clamp(current.fraction + (event.screenY - current.y) / current.height, current.height);
        if (frame.current !== null) return;
        frame.current = requestAnimationFrame(() => {
          frame.current = null;
          if (!drag.current) return;
          setLocalFraction(drag.current.next);
          resize.current(drag.current.next, false);
        });
      }}
      onPointerUp={finish} onPointerCancel={finish} onLostPointerCapture={finish}
      onDoubleClick={event => { event.preventDefault(); commit(DEFAULT_FRACTION); }}
      onKeyDown={event => {
        if (disabled || hidden) return;
        const step = event.shiftKey ? .01 : .05;
        const next = event.key === "ArrowUp" ? displayed - step : event.key === "ArrowDown" ? displayed + step
          : event.key === "Home" ? MIN_FRACTION : event.key === "End" ? MAX_FRACTION : null;
        if (next === null) return;
        event.preventDefault();
        event.stopPropagation();
        commit(next);
      }} />
  </div>;
}
