import { StrictMode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
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
import { applyAppTheme, type AppTheme } from "../shared/theme";

type QuickOpenItem = {
  title: string;
  subtitle: string;
  kind_label: string;
  icon: string;
  color: string;
};

type QuickOpenState = {
  open_id: number;
  theme: AppTheme;
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

declare global {
  interface Window {
    blackholesQuickOpen?: { receive: (event: unknown) => void };
  }
}

function QuickOpenApp() {
  const [state, setState] = useState<QuickOpenState | null>(null);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const selectedRef = useRef<HTMLButtonElement>(null);
  const openIdRef = useRef<number | null>(null);

  useEffect(() => {
    window.blackholesQuickOpen = {
      receive(event) {
        if (typeof event !== "object" || event === null) return;
        const next = event as QuickOpenState & { type?: string };
        if (next.type !== "hydrate") return;
        applyAppTheme(next.theme);
        setState(next);
        if (openIdRef.current !== next.open_id) {
          openIdRef.current = next.open_id;
          setQuery(next.query || "");
          setSelected(0);
        } else {
          setSelected((current) => Math.min(current, Math.max(0, next.results.length - 1)));
        }
      },
    };
    postNative({ type: "ready" });
    return () => { delete window.blackholesQuickOpen; };
  }, []);

  useLayoutEffect(() => {
    if (!state) return;
    inputRef.current?.focus();
  }, [state?.open_id]);

  useEffect(() => {
    selectedRef.current?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  if (!state) return null;

  const dismiss = () => postNative({ type: "dismiss", open_id: state.open_id });
  const activate = (resultIndex: number) => postNative({
    type: "activate",
    open_id: state.open_id,
    result_index: resultIndex,
  });

  return (
    <div className="quick-open-backdrop" onMouseDown={(event) => {
      if (event.target === event.currentTarget) dismiss();
    }}>
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
              postNative({ type: "query_changed", open_id: state.open_id, query: next });
            }}
            onKeyDown={(event) => {
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

const root = document.querySelector("#root");
if (!root) throw new Error("Blackholes quick-open root was not found.");
createRoot(root).render(<StrictMode><QuickOpenApp /></StrictMode>);
