import { useId, useRef, type ReactNode } from "react";

export function Tabs<T extends string>({ value, onValueChange, label, items, disabled = false, initialFocus = false }: {
  value: T; onValueChange: (value: T) => void; label: string;
  items: readonly { value: T; label: ReactNode; content: ReactNode }[];
  disabled?: boolean; initialFocus?: boolean;
}) {
  const id = useId();
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);

  return <div className="tabs">
    <div className="tabs__list" role="tablist" aria-label={label}>
      {items.map((item, index) => <button key={item.value} type="button" role="tab"
        ref={node => { buttons.current[index] = node; }}
        id={`${id}-tab-${item.value}`} aria-controls={`${id}-panel-${item.value}`}
        aria-selected={value === item.value} tabIndex={value === item.value ? 0 : -1}
        disabled={disabled} data-initial-focus={(initialFocus && value === item.value) || undefined}
        onClick={() => onValueChange(item.value)}
        onKeyDown={event => {
          const next = event.key === "ArrowRight" ? (index + 1) % items.length
            : event.key === "ArrowLeft" ? (index + items.length - 1) % items.length
              : event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : null;
          if (next === null || disabled) return;
          event.preventDefault();
          onValueChange(items[next].value);
          buttons.current[next]?.focus();
        }}>{item.label}</button>)}
    </div>
    {items.map(item => <div key={item.value} className="tabs__panel" role="tabpanel"
      id={`${id}-panel-${item.value}`} aria-labelledby={`${id}-tab-${item.value}`}
      hidden={value !== item.value}>{item.content}</div>)}
  </div>;
}
