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
import { createId, postNative } from "../shared/native";
import { TerminalProviderIcon } from "../shared/TerminalProviderIcon";

export type QuickOpenItem = {
  title: string;
  subtitle: string;
  kind_label: string;
  icon: string;
  color: string;
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

// AppKit's NS*ArrowFunctionKey values can arrive from WKWebView as text
// rather than standard DOM arrow key names.
const macArrowKeys = new Map([
  ["\uF700", "ArrowUp"], ["\uF701", "ArrowDown"],
  ["\uF702", "ArrowLeft"], ["\uF703", "ArrowRight"],
]);
const legacyArrowKeys: Record<number, string> = {
  37: "ArrowLeft", 38: "ArrowUp", 39: "ArrowRight", 40: "ArrowDown",
  0xF700: "ArrowUp", 0xF701: "ArrowDown", 0xF702: "ArrowLeft", 0xF703: "ArrowRight",
};
const normalizeSearchKey = (key: string, code = "", keyCode = 0): string => {
  const arrow = macArrowKeys.get(key) || legacyArrowKeys[keyCode];
  if (arrow) return arrow;
  if (key === "Enter") return key;
  return code.startsWith("Key") ? code.slice(3) : code && code !== "Unidentified" ? code : key;
};
const characterSegments = new Intl.Segmenter(undefined, { granularity: "grapheme" });
const wordSegments = new Intl.Segmenter(undefined, { granularity: "word" });
type SearchKeyModifiers = { command?: boolean; shift?: boolean; alt?: boolean };

export function QuickOpen({ state }: { state: QuickOpenState }) {
  const [query, setQuery] = useState(state.query || "");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const selectedRef = useRef<HTMLButtonElement>(null);
  const openIdRef = useRef(state.open_id);
  const pendingPaste = useRef<{ id: string; query: string; start: number; end: number } | null>(null);
  const keyHandler = useRef<(key: string, modifiers?: SearchKeyModifiers) => boolean>(() => false);
  const updateQuery = (next: string) => {
    setQuery(next);
    setSelected(0);
    postNative({ type: "quick_open_query_changed", open_id: state.open_id, query: next });
  };

  useEffect(() => {
    pendingPaste.current = null;
    const receive = (event: Event) => {
      const data = (event as CustomEvent<{ open_id: number; request_id: string; text: string }>).detail;
      const pending = pendingPaste.current;
      const input = inputRef.current;
      if (!pending || !input || data.open_id !== state.open_id || data.request_id !== pending.id) return;
      pendingPaste.current = null;
      // Ignore a late clipboard response if the user has already edited the query.
      if (input.value !== pending.query || !data.text) return;
      const text = data.text.replace(/\r\n?|\n/g, " ");
      updateQuery(pending.query.slice(0, pending.start) + text + pending.query.slice(pending.end));
      requestAnimationFrame(() => {
        input.focus();
        input.setSelectionRange(pending.start + text.length, pending.start + text.length);
      });
    };
    window.addEventListener("blackholes:quick-open-paste", receive);
    return () => window.removeEventListener("blackholes:quick-open-paste", receive);
  }, [state.open_id]);
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

  const handleSearchKey = (rawKey: string, { command = false, shift = false, alt = false }: SearchKeyModifiers = {}): boolean => {
    const input = inputRef.current;
    if (!input) return false;
    const key = normalizeSearchKey(rawKey).toLowerCase();
    if (command && !alt && ["a", "c", "v", "x"].includes(key)) {
      input.focus();
      const start = input.selectionStart ?? 0;
      const end = input.selectionEnd ?? start;
      if (key === "a") input.select();
      else if (key === "v") {
        const id = createId();
        pendingPaste.current = { id, query: input.value, start, end };
        postNative({ type: "quick_open_paste", open_id: state.open_id, request_id: id });
      } else if (start !== end) {
        postNative({ type: "copy_text", text: input.value.slice(start, end) });
        if (key === "x") {
          pendingPaste.current = null;
          updateQuery(input.value.slice(0, start) + input.value.slice(end));
          requestAnimationFrame(() => input.setSelectionRange(start, start));
        }
      }
      return true;
    }
    if (key === "tab") input.focus();
    else if (key === "arrowup") setSelected(current => Math.max(0, current - 1));
    else if (key === "arrowdown") setSelected(current => Math.min(Math.max(0, state.results.length - 1), current + 1));
    else if (key === "arrowleft" || key === "arrowright") {
      const left = key === "arrowleft";
      const start = input.selectionStart ?? 0, end = input.selectionEnd ?? start;
      const backward = input.selectionDirection === "backward";
      const anchor = backward ? end : start;
      const position = shift ? (backward ? start : end) : left ? start : end;
      let caret = position;
      if (command) caret = left ? 0 : input.value.length;
      else if (alt) {
        const words = [...wordSegments.segment(input.value)].filter(segment => segment.isWordLike);
        const word = left ? words.filter(segment => segment.index < position).at(-1)
          : words.find(segment => segment.index + segment.segment.length > position);
        caret = word ? word.index + (left ? 0 : word.segment.length) : left ? 0 : input.value.length;
      } else if (shift || start === end) {
        const boundaries = [0, ...[...characterSegments.segment(input.value)].map(segment => segment.index + segment.segment.length)];
        caret = left ? boundaries.filter(index => index < position).at(-1) ?? 0
          : boundaries.find(index => index > position) ?? input.value.length;
      }
      input.focus();
      input.setSelectionRange(shift ? Math.min(anchor, caret) : caret, shift ? Math.max(anchor, caret) : caret,
        shift && caret < anchor ? "backward" : "forward");
    } else if (key === "enter") {
      if (state.results[selected]) activate(selected);
    } else if (key === "escape") dismiss();
    else return false;
    return true;
  };
  useLayoutEffect(() => { keyHandler.current = handleSearchKey; });
  useEffect(() => {
    const receive = (event: Event) => {
      const data = (event as CustomEvent<{ open_id: number; key: string } & SearchKeyModifiers>).detail;
      if (data.open_id === state.open_id) keyHandler.current(data.key, data);
    };
    window.addEventListener("blackholes:quick-open-key", receive);
    return () => window.removeEventListener("blackholes:quick-open-key", receive);
  }, [state.open_id]);
  useEffect(() => {
    const input = inputRef.current;
    if (!input) return;
    const beforeInput = (event: InputEvent) => {
      if (event.inputType !== "insertText" || !event.data || !event.cancelable) return;
      const characters = [...event.data];
      if (!characters.every(character => macArrowKeys.has(character))) return;
      event.preventDefault();
      for (const character of characters) keyHandler.current(macArrowKeys.get(character)!);
    };
    input.addEventListener("beforeinput", beforeInput);
    return () => input.removeEventListener("beforeinput", beforeInput);
  }, [state.open_id]);

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
              pendingPaste.current = null;
              const input = event.currentTarget;
              const next = input.value.replace(/[\uF700-\uF703]/g, "");
              if (next !== input.value) {
                // Some native text commits cannot cancel beforeinput. Restore
                // the query and handle the function characters as navigation.
                const arrows = [...input.value].filter(character => macArrowKeys.has(character));
                const caret = input.value.slice(0, input.selectionStart ?? 0).replace(/[\uF700-\uF703]/g, "").length;
                input.value = next;
                input.setSelectionRange(caret, caret);
                if (next !== query) updateQuery(next);
                for (const character of arrows) keyHandler.current(macArrowKeys.get(character)!);
              } else updateQuery(next);
            }}
            onKeyDown={(event) => {
              event.stopPropagation();
              if (event.nativeEvent.isComposing) return;
              const key = normalizeSearchKey(event.key, event.code, event.keyCode);
              if (handleSearchKey(key, { command: event.metaKey || event.ctrlKey, shift: event.shiftKey, alt: event.altKey })) event.preventDefault();
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
                    {item.terminal_provider ? <TerminalProviderIcon provider={item.terminal_provider} /> : <Icon size={17} />}
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
