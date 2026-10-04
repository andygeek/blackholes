import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { Check, ChevronDown } from "lucide-react";

type Option<T extends string> = { value: T; label: string; description: string };

/** Keeps the menu inside the modal's focus boundary instead of opening a system picker. */
export function ModalSelect<T extends string>({ label, description, value, options, onValueChange, disabled = false }: {
  label: string; description?: string; value: T; options: readonly Option<T>[];
  onValueChange: (value: T) => void; disabled?: boolean;
}) {
  const id = useId();
  const container = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const search = useRef({ text: "", time: 0 });
  const selected = Math.max(0, options.findIndex(option => option.value === value));
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(selected);
  const [placement, setPlacement] = useState({ above: false, maxHeight: 240 });

  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);

  useLayoutEffect(() => {
    if (!open) return;
    const position = () => {
      if (!trigger.current || !menu.current) return;
      const bounds = trigger.current.getBoundingClientRect();
      const modal = trigger.current.closest(".app-modal")?.getBoundingClientRect();
      const below = Math.max(0, Math.min(window.innerHeight - 12, modal ? modal.bottom - 10 : Infinity) - bounds.bottom - 6);
      const above = Math.max(0, bounds.top - Math.max(12, modal ? modal.top + 10 : 0) - 6);
      const placeAbove = menu.current.scrollHeight > below && above > below;
      setPlacement({ above: placeAbove, maxHeight: Math.min(280, placeAbove ? above : below) });
    };
    const outside = (event: PointerEvent) => {
      if (!container.current?.contains(event.target as Node)) setOpen(false);
    };
    const onScroll = (event: Event) => {
      if (!menu.current?.contains(event.target as Node)) position();
    };
    position();
    document.addEventListener("pointerdown", outside, true);
    window.addEventListener("resize", position);
    document.addEventListener("scroll", onScroll, true);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("resize", position);
      document.removeEventListener("scroll", onScroll, true);
    };
  }, [open]);

  useEffect(() => {
    if (open) document.getElementById(`${id}-option-${active}`)?.scrollIntoView({ block: "nearest" });
  }, [open, active, id]);

  const show = (index = selected) => {
    search.current = { text: "", time: 0 };
    setActive(index);
    setOpen(true);
  };
  const choose = (index: number) => {
    if (disabled || !options[index]) return;
    onValueChange(options[index].value);
    setOpen(false);
    trigger.current?.focus();
  };

  return <div ref={container} className="modal-select" data-open={open || undefined}>
    <span id={`${id}-label`} className="modal-select__label">{label}</span>
    <div className="modal-select__control">
    <button ref={trigger} type="button" className="modal-select__trigger" role="combobox"
      disabled={disabled} aria-expanded={open} aria-haspopup="listbox" aria-controls={`${id}-menu`}
      aria-labelledby={`${id}-label ${id}-value`} aria-describedby={description ? `${id}-hint` : undefined}
      aria-activedescendant={open ? `${id}-option-${active}` : undefined}
      onClick={() => { trigger.current?.focus(); if (open) setOpen(false); else show(); }}
      onBlur={event => { if (!container.current?.contains(event.relatedTarget as Node | null)) setOpen(false); }}
      onKeyDown={event => {
        if (disabled || event.nativeEvent.isComposing) return;
        if (event.key === "Tab") { setOpen(false); return; }
        if (event.key === "Escape" && open) {
          event.preventDefault(); event.stopPropagation(); setOpen(false); return;
        }
        if (["ArrowDown", "ArrowUp", "Home", "End", "Enter", " "].includes(event.key)) {
          event.preventDefault(); event.stopPropagation();
          if (event.key === "Enter" || event.key === " ") { if (open) choose(active); else show(); }
          else if (event.key === "Home" || event.key === "End") {
            const index = event.key === "Home" ? 0 : options.length - 1;
            if (open) setActive(index); else show(index);
          } else if (!open) show();
          else setActive(current => Math.max(0, Math.min(options.length - 1, current + (event.key === "ArrowDown" ? 1 : -1))));
          return;
        }
        if (event.key.length !== 1 || event.metaKey || event.ctrlKey || event.altKey) return;
        event.preventDefault(); event.stopPropagation();
        const now = Date.now();
        const query = now - search.current.time < 700 ? search.current.text + event.key.toLocaleLowerCase() : event.key.toLocaleLowerCase();
        const prefix = [...query].every(character => character === query[0]) ? query[0] : query;
        const start = open ? active : selected;
        const index = options.findIndex((_, offset) => options[(start + offset + 1) % options.length].label.toLocaleLowerCase().startsWith(prefix));
        if (index >= 0) { const next = (start + index + 1) % options.length; if (open) setActive(next); else show(next); }
        search.current = { text: query, time: now };
      }}>
      <span id={`${id}-value`}>{options[selected]?.label}</span><ChevronDown size={14} aria-hidden="true" />
    </button>
    {open && <div ref={menu} id={`${id}-menu`} className="modal-select__menu" role="listbox"
      aria-labelledby={`${id}-label`} data-above={placement.above || undefined} style={{ maxHeight: placement.maxHeight }}>
      {options.map((option, index) => <div key={option.value} id={`${id}-option-${index}`} className="modal-select__option"
        role="option" aria-selected={option.value === value} data-active={index === active || undefined}
        onPointerMove={() => setActive(index)} onMouseDown={event => event.preventDefault()} onClick={() => choose(index)}>
        <span><span className="modal-select__option-label">{option.label}</span><small>{option.description}</small></span>
        <Check size={14} aria-hidden="true" style={{ visibility: option.value === value ? "visible" : "hidden" }} />
      </div>)}
    </div>}
    </div>
    {description && <small id={`${id}-hint`} className="modal-select__hint">{description}</small>}
  </div>;
}
