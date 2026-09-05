import { useLayoutEffect, useRef, useState, type ReactNode } from "react";

/** An overlaid thumb keeps section borders full-width instead of reserving a gutter. */
export function SidebarScrollArea({ children, label }: { children: ReactNode; label: string }) {
  const viewport = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const drag = useRef<{ y: number; top: number; ratio: number } | null>(null);
  const [metrics, setMetrics] = useState({ height: 0, total: 0, top: 0 });
  useLayoutEffect(() => {
    const element = viewport.current!;
    let frame: number | null = null;
    const measure = () => {
      frame = null;
      const next = { height: element.clientHeight, total: element.scrollHeight, top: element.scrollTop };
      setMetrics((previous) => previous.height === next.height && previous.total === next.total && previous.top === next.top ? previous : next);
    };
    const schedule = () => { if (frame === null) frame = requestAnimationFrame(measure); };
    const observer = new ResizeObserver(schedule);
    observer.observe(element);
    observer.observe(content.current!);
    element.addEventListener("scroll", schedule, { passive: true });
    measure();
    return () => {
      observer.disconnect();
      element.removeEventListener("scroll", schedule);
      if (frame !== null) cancelAnimationFrame(frame);
    };
  }, []);
  const maxScroll = Math.max(0, metrics.total - metrics.height);
  const thumbHeight = Math.min(metrics.height, Math.max(28, metrics.height * metrics.height / Math.max(1, metrics.total)));
  const travel = metrics.height - thumbHeight;
  const thumbTop = maxScroll > 0 ? Math.max(0, Math.min(travel, metrics.top / maxScroll * travel)) : 0;
  return <div className="sidebar-scroll-region">
    <div className="sidebar-scroll" id="sidebar-scroll-content" ref={viewport}>
      <div ref={content}>{children}</div>
    </div>
    {maxScroll > 1 && <div className="sidebar-scroll-track"
      onWheel={(event) => {
        if (viewport.current) viewport.current.scrollTop += event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? metrics.height : 1);
      }}>
      <div className="sidebar-scroll-thumb" role="scrollbar" tabIndex={0}
        aria-label={label} aria-controls="sidebar-scroll-content" aria-orientation="vertical"
        aria-valuemin={0} aria-valuemax={Math.round(maxScroll)} aria-valuenow={Math.round(Math.max(0, Math.min(maxScroll, metrics.top)))}
        style={{ height: thumbHeight, transform: `translateY(${thumbTop}px)` }}
        onPointerDown={(event) => {
          if (event.button !== 0 || travel <= 0) return;
          event.preventDefault();
          event.stopPropagation();
          event.currentTarget.setPointerCapture(event.pointerId);
          drag.current = { y: event.screenY, top: viewport.current?.scrollTop ?? 0, ratio: maxScroll / travel };
        }}
        onPointerMove={(event) => {
          if (drag.current && viewport.current) viewport.current.scrollTop = drag.current.top + (event.screenY - drag.current.y) * drag.current.ratio;
        }}
        onPointerUp={() => { drag.current = null; }}
        onPointerCancel={() => { drag.current = null; }}
        onLostPointerCapture={() => { drag.current = null; }}
        onKeyDown={(event) => {
          const element = viewport.current;
          if (!element) return;
          const next = event.key === "ArrowDown" ? element.scrollTop + 40 : event.key === "ArrowUp" ? element.scrollTop - 40
            : event.key === "PageDown" ? element.scrollTop + metrics.height : event.key === "PageUp" ? element.scrollTop - metrics.height
              : event.key === "Home" ? 0 : event.key === "End" ? maxScroll : null;
          if (next === null) return;
          event.preventDefault();
          event.stopPropagation();
          element.scrollTop = next;
        }} />
    </div>}
  </div>;
}
