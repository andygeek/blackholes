import { useEffect, useRef, useState, type KeyboardEvent, type MouseEvent, type PointerEvent } from "react";

/** What is being dragged in the Agents list. */
export type DragItem = { kind: "session"; id: string } | { kind: "section"; id: string };

/** Where it would land. Rows and headers carry the data attributes read here. */
export type DropTarget =
  | { kind: "session-row"; terminalId: string; sectionId: string | null; after: boolean }
  | { kind: "section"; sectionId: string; position: "before" | "after" | "inside" }
  | { kind: "unsectioned" };

interface Drag {
  item: DragItem;
  pointer: number;
  startX: number;
  startY: number;
  x: number;
  y: number;
  active: boolean;
  moved: boolean;
}

const HOLD_DELAY_MS = 400;
const MOVEMENT_TOLERANCE = 6;
const ROWS = "[data-drag-session],[data-drag-section],[data-drop-unsectioned]";

const sameTarget = (a: DropTarget | null, b: DropTarget | null) => JSON.stringify(a) === JSON.stringify(b);

/** Press and hold a session or a section header, then drag it: sessions move between sections
 * and reorder; sections reorder or go inside another one. A quick press stays a click. */
export function useAgentDrag({ disabled, allows, onDrop, onMoveByKey }: {
  disabled: boolean;
  /** Whether the item may land on this target, such as a section inside its own sub-section. */
  allows(item: DragItem, target: DropTarget): boolean;
  onDrop(item: DragItem, target: DropTarget): void;
  /** ⌥↑ / ⌥↓ on a focused session or section header. */
  onMoveByKey(item: DragItem, direction: -1 | 1): void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const holdTimer = useRef<number | null>(null);
  const drop = useRef<DropTarget | null>(null);
  const suppressClick = useRef(false);
  const [dragging, setDragging] = useState<DragItem | null>(null);
  const [target, setTarget] = useState<DropTarget | null>(null);
  const callbacks = useRef({ allows, onDrop });
  callbacks.current = { allows, onDrop };

  const targetAt = (current: Drag): DropTarget | null => {
    const list = container.current;
    const viewport = list?.closest<HTMLElement>(".sidebar-scroll");
    const bounds = viewport?.getBoundingClientRect();
    if (!list || !bounds || current.x < bounds.left || current.x > bounds.right || current.y < bounds.top || current.y > bounds.bottom) return null;
    const rows = [...list.querySelectorAll<HTMLElement>(ROWS)];
    // The row under the pointer, or the nearest one when it is in a gap between rows.
    let row: HTMLElement | undefined;
    let distance = Infinity;
    for (const candidate of rows) {
      const rect = candidate.getBoundingClientRect();
      if (current.y >= rect.top && current.y < rect.bottom) { row = candidate; break; }
      const gap = Math.min(Math.abs(current.y - rect.top), Math.abs(current.y - rect.bottom));
      if (gap < distance) { distance = gap; row = candidate; }
    }
    if (!row) return null;
    const rect = row.getBoundingClientRect();
    const offset = (current.y - rect.top) / Math.max(rect.height, 1);
    let next: DropTarget;
    if (row.dataset.dropUnsectioned !== undefined) {
      next = { kind: "unsectioned" };
    } else if (row.dataset.dragSection) {
      const sectionId = row.dataset.dragSection;
      next = current.item.kind === "session"
        ? { kind: "section", sectionId, position: "inside" }
        : { kind: "section", sectionId, position: offset < .3 ? "before" : offset > .7 ? "after" : "inside" };
    } else {
      const terminalId = row.dataset.dragSession!;
      const sectionId = row.dataset.sectionId || null;
      if (current.item.kind === "session") {
        if (terminalId === current.item.id) return null;
        next = { kind: "session-row", terminalId, sectionId, after: offset > .5 };
      } else {
        // A section dropped on a session goes inside that session's section.
        next = sectionId ? { kind: "section", sectionId, position: "inside" } : { kind: "unsectioned" };
      }
    }
    return callbacks.current.allows(current.item, next) ? next : null;
  };

  const updateTarget = () => {
    const current = drag.current;
    if (!current?.active) return;
    const next = targetAt(current);
    drop.current = next;
    setTarget(previous => sameTarget(previous, next) ? previous : next);
  };

  const cancel = () => {
    if (holdTimer.current !== null) window.clearTimeout(holdTimer.current);
    holdTimer.current = null;
    const pointer = drag.current?.pointer;
    drag.current = null;
    drop.current = null;
    setDragging(null);
    setTarget(null);
    if (pointer !== undefined && container.current?.hasPointerCapture(pointer)) container.current.releasePointerCapture(pointer);
  };

  useEffect(() => {
    if (disabled) cancel();
  }, [disabled]);

  useEffect(() => {
    const escape = (event: globalThis.KeyboardEvent) => {
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

  // Scrolls near the top or bottom edge, so a drag can reach rows out of view.
  useEffect(() => {
    if (!dragging) return;
    let frame = 0;
    let previousTime = 0;
    const scroll = (time: number) => {
      const current = drag.current;
      const viewport = container.current?.closest<HTMLElement>(".sidebar-scroll");
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

  const itemOf = (element: Element): DragItem | null => {
    const session = element.closest(".agent-open")?.closest<HTMLElement>("[data-drag-session]");
    if (session) return { kind: "session", id: session.dataset.dragSession! };
    const section = element.closest(".agent-section-toggle")?.closest<HTMLElement>("[data-drag-section]");
    return section ? { kind: "section", id: section.dataset.dragSection! } : null;
  };

  const handlers = {
    onDragStart: (event: { preventDefault(): void }) => event.preventDefault(),
    onPointerDown: (event: PointerEvent<HTMLDivElement>) => {
      suppressClick.current = false;
      if (disabled || event.button !== 0 || !event.isPrimary || event.pointerType === "touch") return;
      // The X, the pin and the options stay normal buttons, never drag handles.
      const item = itemOf(event.target as Element);
      if (!item) return;
      cancel();
      const current: Drag = { item, pointer: event.pointerId, startX: event.clientX, startY: event.clientY, x: event.clientX, y: event.clientY, active: false, moved: false };
      drag.current = current;
      holdTimer.current = window.setTimeout(() => {
        holdTimer.current = null;
        if (drag.current !== current || !container.current) return;
        container.current.setPointerCapture(current.pointer);
        current.active = true;
        suppressClick.current = true;
        setDragging(current.item);
      }, HOLD_DELAY_MS);
    },
    onPointerMove: (event: PointerEvent<HTMLDivElement>) => {
      const current = drag.current;
      if (!current || current.pointer !== event.pointerId) return;
      if ((event.buttons & 1) === 0) { cancel(); return; }
      current.x = event.clientX;
      current.y = event.clientY;
      if (Math.hypot(current.x - current.startX, current.y - current.startY) >= MOVEMENT_TOLERANCE) {
        // Moving before the hold finishes is a scroll or a click, never an accidental move.
        if (!current.active) { cancel(); return; }
        current.moved = true;
      }
      if (current.active) { event.preventDefault(); if (current.moved) updateTarget(); }
    },
    onPointerUp: (event: PointerEvent<HTMLDivElement>) => {
      const current = drag.current;
      if (!current || current.pointer !== event.pointerId) return;
      if (current.active) {
        event.preventDefault();
        if (current.moved) updateTarget();
        const destination = drop.current;
        if (current.moved && destination) callbacks.current.onDrop(current.item, destination);
      }
      cancel();
    },
    onPointerLeave: () => { if (!drag.current?.active) cancel(); },
    onPointerCancel: cancel,
    onLostPointerCapture: cancel,
    onClickCapture: (event: MouseEvent<HTMLDivElement>) => {
      if (suppressClick.current) { event.preventDefault(); event.stopPropagation(); suppressClick.current = false; }
    },
    onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
      if (disabled || !event.altKey || !["ArrowUp", "ArrowDown"].includes(event.key)) return;
      const item = itemOf(event.target as Element);
      if (!item) return;
      event.preventDefault();
      event.stopPropagation();
      onMoveByKey(item, event.key === "ArrowUp" ? -1 : 1);
    },
  };

  return { container, dragging, target, handlers };
}
