import { useEffect, useRef, useState, type ReactNode } from "react";

interface AgentEntry { id: string; label: string; content: ReactNode }
interface DropTarget { id: string; after: boolean }
interface Drag { id: string; pointer: number; startX: number; startY: number; x: number; y: number; active: boolean; moved: boolean }

const HOLD_DELAY_MS = 400;
const MOVEMENT_TOLERANCE = 6;

/** Reorder cards without opening a terminal, changing project ownership or moving the tree. */
export function SortableAgentList({ items, disabled, language, onReorder }: {
  items: AgentEntry[]; disabled: boolean; language: "en" | "es"; onReorder(ids: string[]): void;
}) {
  const list = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const holdTimer = useRef<number | null>(null);
  const drop = useRef<DropTarget | null>(null);
  const suppressClick = useRef(false);
  const [dragging, setDragging] = useState<string | null>(null);
  const [target, setTarget] = useState<DropTarget | null>(null);
  const [announcement, setAnnouncement] = useState("");

  const updateTarget = () => {
    const current = drag.current;
    if (!current?.active || !list.current) return;
    const viewport = list.current.closest<HTMLElement>(".sidebar-scroll");
    const bounds = viewport?.getBoundingClientRect();
    let next: DropTarget | null = null;
    if (bounds && current.x >= bounds.left && current.x <= bounds.right && current.y >= bounds.top && current.y <= bounds.bottom) {
      const rows = [...list.current.querySelectorAll<HTMLElement>("[data-agent-id]")];
      // Include row gaps and the end of the list as valid insertion positions.
      const row = rows.find(row => {
        const rect = row.getBoundingClientRect();
        return current.y < rect.top + rect.height / 2;
      });
      const last = rows.at(-1);
      if (row) next = { id: row.dataset.agentId!, after: false };
      else if (last) next = { id: last.dataset.agentId!, after: true };
    }
    drop.current = next;
    setTarget(previous => previous?.id === next?.id && previous?.after === next?.after ? previous : next);
  };

  const cancel = () => {
    if (holdTimer.current !== null) window.clearTimeout(holdTimer.current);
    holdTimer.current = null;
    const pointer = drag.current?.pointer;
    drag.current = null;
    drop.current = null;
    setDragging(null);
    setTarget(null);
    if (pointer !== undefined && list.current?.hasPointerCapture(pointer)) list.current.releasePointerCapture(pointer);
  };

  useEffect(() => {
    if (disabled) cancel();
  }, [disabled]);

  useEffect(() => {
    if (drag.current && !items.some(item => item.id === drag.current?.id)) cancel();
  }, [items]);

  useEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && drag.current) {
        event.preventDefault();
        cancel();
      }
    };
    window.addEventListener("blur", cancel);
    window.addEventListener("keydown", escape);
    return () => {
      window.removeEventListener("blur", cancel);
      window.removeEventListener("keydown", escape);
      if (holdTimer.current !== null) window.clearTimeout(holdTimer.current);
    };
  }, []);

  useEffect(() => {
    if (!dragging) return;
    let frame = 0;
    let previousTime = 0;
    const scroll = (time: number) => {
      const current = drag.current;
      const viewport = list.current?.closest<HTMLElement>(".sidebar-scroll");
      if (current?.active && current.moved && viewport) {
        const bounds = viewport.getBoundingClientRect();
        if (current.x >= bounds.left && current.x <= bounds.right && current.y >= bounds.top && current.y <= bounds.bottom) {
          const edge = Math.min(40, bounds.height / 4);
          const speed = current.y < bounds.top + edge ? -1 : current.y > bounds.bottom - edge ? 1 : 0;
          viewport.scrollTop += speed * Math.min(32, previousTime ? time - previousTime : 16) * .4;
        }
        updateTarget();
      }
      previousTime = time;
      frame = requestAnimationFrame(scroll);
    };
    frame = requestAnimationFrame(scroll);
    return () => cancelAnimationFrame(frame);
  }, [dragging]);

  const commit = (ids: string[], id: string) => {
    if (ids.every((key, index) => key === items[index]?.id)) return;
    onReorder(ids);
    const name = items.find(item => item.id === id)?.label || "";
    setAnnouncement(language === "es" ? `${name}: posición ${ids.indexOf(id) + 1} de ${ids.length}` : `${name}: position ${ids.indexOf(id) + 1} of ${ids.length}`);
  };

  return <div ref={list} className={`global-agents${dragging ? " is-reordering" : ""}`}
    onDragStart={event => event.preventDefault()}
    onPointerDown={event => {
      suppressClick.current = false;
      if (disabled || event.button !== 0 || !event.isPrimary || event.pointerType === "touch") return;
      const button = (event.target as Element).closest(".agent-open");
      const row = button?.closest<HTMLElement>("[data-agent-id]");
      if (!row) return; // The X remains a normal confirmation button, never a drag handle.
      cancel();
      const current: Drag = { id: row.dataset.agentId!, pointer: event.pointerId, startX: event.clientX, startY: event.clientY, x: event.clientX, y: event.clientY, active: false, moved: false };
      drag.current = current;
      holdTimer.current = window.setTimeout(() => {
        holdTimer.current = null;
        if (drag.current !== current || !list.current) return;
        list.current.setPointerCapture(current.pointer);
        current.active = true;
        suppressClick.current = true;
        setDragging(current.id);
      }, HOLD_DELAY_MS);
    }}
    onPointerMove={event => {
      const current = drag.current;
      if (!current || current.pointer !== event.pointerId) return;
      if ((event.buttons & 1) === 0) { cancel(); return; }
      current.x = event.clientX;
      current.y = event.clientY;
      if (Math.hypot(current.x - current.startX, current.y - current.startY) >= MOVEMENT_TOLERANCE) {
        // Moving before the hold finishes must not start an accidental reorder.
        if (!current.active) { cancel(); return; }
        current.moved = true;
      }
      if (current.active) { event.preventDefault(); if (current.moved) updateTarget(); }
    }}
    onPointerUp={event => {
      const current = drag.current;
      if (!current || current.pointer !== event.pointerId) return;
      if (current.active) {
        event.preventDefault();
        if (current.moved) updateTarget();
        const destination = drop.current;
        if (destination && destination.id !== current.id && items.some(item => item.id === destination.id)) {
          const ids = items.map(item => item.id).filter(id => id !== current.id);
          ids.splice(ids.indexOf(destination.id) + Number(destination.after), 0, current.id);
          commit(ids, current.id);
        }
      }
      cancel();
    }}
    onPointerLeave={() => { if (!drag.current?.active) cancel(); }}
    onPointerCancel={cancel} onLostPointerCapture={cancel}
    onClickCapture={event => {
      if (suppressClick.current) { event.preventDefault(); event.stopPropagation(); suppressClick.current = false; }
    }}
    onKeyDown={event => {
      if (disabled || !event.altKey || !["ArrowUp", "ArrowDown"].includes(event.key) || !(event.target as Element).closest(".agent-open")) return;
      const id = (event.target as Element).closest<HTMLElement>("[data-agent-id]")?.dataset.agentId;
      if (!id) return;
      event.preventDefault();
      event.stopPropagation();
      const ids = items.map(item => item.id);
      const index = ids.indexOf(id), next = index + (event.key === "ArrowUp" ? -1 : 1);
      if (next >= 0 && next < ids.length) { [ids[index], ids[next]] = [ids[next], ids[index]]; commit(ids, id); }
    }}>
    {items.map(item => <div key={item.id} data-agent-id={item.id}
      className={`sortable-agent${dragging === item.id ? " is-dragging" : ""}${target?.id === item.id && target.id !== dragging ? target.after ? " drop-after" : " drop-before" : ""}`}
      title={language === "es" ? "Mantén pulsado y arrastra para ordenar · ⌥↑ / ⌥↓" : "Press and hold, then drag to reorder · ⌥↑ / ⌥↓"}>
      {item.content}
    </div>)}
    <span className="agent-order-announcement" role="status" aria-live="polite">{announcement}</span>
  </div>;
}
