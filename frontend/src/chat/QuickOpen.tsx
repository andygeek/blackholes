import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  Code2,
  Database,
  File,
  Folder,
  GitBranch,
  Globe2,
  Layers3,
  ListTodo,
  Rocket,
  Search,
  SquareTerminal,
  type LucideIcon,
} from "lucide-react";
import { postNative } from "../shared/native";

export type QuickOpenItem = {
  title: string;
  subtitle: string;
  kind_label: string;
  icon: string;
  color: string;
};

export type QuickOpenState = {
  open_id: number;
  query: string;
  placeholder: string;
  shortcut: string;
  footer_label: string;
  navigation_label: string;
  status?: string | null;
  error?: boolean;
  results: QuickOpenItem[];
};

const icons: Record<string, LucideIcon> = {
  code: Code2,
  database: Database,
  file: File,
  folder: Folder,
  globe: Globe2,
  layers: Layers3,
  list: ListTodo,
  rocket: Rocket,
  terminal: SquareTerminal,
  branch: GitBranch,
};

export function QuickOpen({ state }: { state: QuickOpenState }) {
  const [query, setQuery] = useState(state.query || "");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const selectedRef = useRef<HTMLButtonElement>(null);
  const openIdRef = useRef(state.open_id);

  useEffect(() => {
    if (openIdRef.current !== state.open_id) {
      openIdRef.current = state.open_id;
      setQuery(state.query || "");
      setSelected(0);
      return;
    }
    setSelected((current) => Math.min(current, Math.max(0, state.results.length - 1)));
  }, [state.open_id, state.query, state.results.length]);

  useLayoutEffect(() => {
    inputRef.current?.focus();
  }, [state.open_id]);

  useEffect(() => {
    selectedRef.current?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const dismiss = () => postNative({ type: "quick_open_dismiss", open_id: state.open_id });
  const activate = (resultIndex: number) => postNative({
    type: "quick_open_activate",
    open_id: state.open_id,
    result_index: resultIndex,
  });

  return (
    <div
      className="quick-open-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) dismiss();
      }}
    >
      <section className="quick-open-panel" role="dialog" aria-modal="true" aria-label={state.placeholder}>
        <div className="quick-open-search">
          <Search size={19} aria-hidden="true" />
          <input
            ref={inputRef}
            value={query}
            placeholder={state.placeholder}
            aria-label={state.placeholder}
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => {
              const next = event.target.value;
              setQuery(next);
              setSelected(0);
              postNative({ type: "quick_open_query_changed", open_id: state.open_id, query: next });
            }}
            onKeyDown={(event) => {
              event.stopPropagation();
              if (event.key === "ArrowUp") {
                event.preventDefault();
                setSelected((current) => Math.max(0, current - 1));
              } else if (event.key === "ArrowDown") {
                event.preventDefault();
                setSelected((current) => Math.min(state.results.length - 1, current + 1));
              } else if (event.key === "Enter" && state.results[selected]) {
                event.preventDefault();
                activate(selected);
              } else if (event.key === "Escape") {
                event.preventDefault();
                dismiss();
              }
            }}
          />
          <span className="quick-open-shortcut">{state.shortcut}</span>
        </div>

        {state.status || state.results.length === 0 ? (
          <div className={`quick-open-empty${state.error ? " is-error" : ""}`}>
            {state.status}
          </div>
        ) : (
          <div className="quick-open-results" role="listbox">
            {state.results.map((item, index) => {
              const Icon = icons[item.icon] || File;
              return (
                <button
                  ref={index === selected ? selectedRef : undefined}
                  key={`${item.title}-${item.subtitle}-${index}`}
                  type="button"
                  role="option"
                  aria-selected={index === selected}
                  className={`quick-open-row${index === selected ? " is-selected" : ""}`}
                  onMouseEnter={() => setSelected(index)}
                  onClick={() => activate(index)}
                >
                  <span className="quick-open-row__icon" style={{ color: item.color }}><Icon size={17} /></span>
                  <span className="quick-open-row__title">{item.title}</span>
                  <span className="quick-open-row__subtitle">{item.subtitle}</span>
                  <span className="quick-open-row__kind">{item.kind_label}</span>
                </button>
              );
            })}
          </div>
        )}

        <footer className="quick-open-footer">
          <span>{state.navigation_label}</span>
          <span>{state.footer_label}</span>
        </footer>
      </section>
    </div>
  );
}
