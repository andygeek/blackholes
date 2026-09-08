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
import { AgentAvatar } from "../shared/AgentAvatar";
import { TerminalProviderIcon } from "../shared/TerminalProviderIcon";

export type QuickOpenItem = {
  title: string;
  subtitle: string;
  kind_label: string;
  icon: string;
  color: string;
  agent_identity?: string | null;
  terminal_provider?: string | null;
};

export type QuickOpenState = {
  open_id: number;
  sidebar_width?: number;
  over_terminal?: boolean;
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
  const [layout, setLayout] = useState({ width: 0, offset: 0 });

  useLayoutEffect(() => {
    const resize = () => {
      // Account for the sidebar outside this WebView while keeping the panel
      // within its visible bounds when the app window is narrow.
      const sidebar = state.sidebar_width || 0;
      const available = Math.max(0, window.innerWidth - 56);
      const width = Math.min(760, available, Math.max(260, available - sidebar));
      const offset = -Math.min(sidebar / 2, Math.max(0, (available - width) / 2));
      setLayout({ width, offset });
    };
    window.addEventListener("resize", resize);
    resize();
    return () => window.removeEventListener("resize", resize);
  }, [state.sidebar_width]);

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
  const activate = (resultIndex: number) => query === state.query && postNative({
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
      <section className={`quick-open-panel${layout.width < 640 ? " is-compact" : ""}`} role="dialog" aria-modal="true" aria-label={state.placeholder}
        style={{ width: layout.width || undefined, transform: `translateX(${layout.offset}px)` }}
        onKeyDown={event => {
          if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); dismiss(); }
          if (event.key === "Tab") { event.preventDefault(); inputRef.current?.focus(); }
        }}>
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
              if (event.key === "Tab") {
                event.preventDefault();
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setSelected((current) => Math.max(0, current - 1));
              } else if (event.key === "ArrowDown") {
                event.preventDefault();
                setSelected((current) => Math.min(Math.max(0, state.results.length - 1), current + 1));
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
                  <span className="quick-open-row__icon" style={{ color: item.color }}>
                    {item.agent_identity ? <AgentAvatar identity={item.agent_identity} size={24} />
                      : item.terminal_provider ? <TerminalProviderIcon provider={item.terminal_provider} /> : <Icon size={17} />}
                  </span>
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
