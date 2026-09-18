import { useEffect, useState } from "react";
import { CaseSensitive, ChevronDown, ChevronRight, FileCode2, ListFilter, LoaderCircle, Regex, Search, WholeWord, X } from "lucide-react";
import { createId, postNative } from "../shared/native";

interface Options { query: string; case_sensitive: boolean; whole_word: boolean; regex: boolean; include: string; exclude: string }
interface Match { line: number; column: number; end_column: number; before: string; matched: string; after: string }
interface FileMatches { path: string; matches: Match[] }
export interface SearchData {
  request_id: string; options: Options | null; loading: boolean; error: string | null;
  result: { files: FileMatches[]; count: number; limited: boolean; skipped: number } | null;
}
const drafts = new Map<string, Options>();
const empty: Options = { query: "", case_sensitive: false, whole_word: false, regex: false, include: "", exclude: "" };

export function RepositorySearch({ root, language, search }: { root: string; language: "en" | "es"; search?: SearchData }) {
  const tr = (en: string, es: string) => language === "en" ? en : es;
  const [options, setOptions] = useState(() => drafts.get(root) || search?.options || empty);
  const [filters, setFilters] = useState(!!options.include || !!options.exclude);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [limit, setLimit] = useState(200);
  useEffect(() => {
    drafts.set(root, options);
    const timer = window.setTimeout(() => {
      postNative({ type: "search_repository", root_path: root, request_id: createId(), options });
      setLimit(200);
    }, 300);
    return () => window.clearTimeout(timer);
  }, [root, options]);
  const sameQuery = !!search?.options && Object.keys(empty).every(key => search.options![key as keyof Options] === options[key as keyof Options]);
  const loading = !!options.query && (!sameQuery || !!search?.loading);
  const result = sameQuery && !loading ? search?.result : null;
  const update = (patch: Partial<Options>) => setOptions(current => ({ ...current, ...patch }));
  return <section className="repository-search">
    <div className="repository-search-controls">
      <div className="repository-search-input"><Search size={14} /><input autoFocus value={options.query} placeholder={tr("Search in repository", "Buscar en el repositorio")} aria-label={tr("Search in repository", "Buscar en el repositorio")} maxLength={4096}
        onChange={e => update({ query: e.target.value })} onKeyDown={e => { if (e.key === "Enter") postNative({ type: "search_repository", root_path: root, request_id: createId(), options }); }} />
        {options.query && <button aria-label={tr("Clear search", "Limpiar búsqueda")} title={tr("Clear search", "Limpiar búsqueda")} onClick={() => update({ query: "" })}><X size={13} /></button>}
      </div>
      <div className="repository-search-options">
        <button aria-pressed={options.case_sensitive} title={tr("Match case", "Distinguir mayúsculas")} aria-label={tr("Match case", "Distinguir mayúsculas")} onClick={() => update({ case_sensitive: !options.case_sensitive })}><CaseSensitive size={16} /></button>
        <button aria-pressed={options.whole_word} title={tr("Match whole word", "Palabra completa")} aria-label={tr("Match whole word", "Palabra completa")} onClick={() => update({ whole_word: !options.whole_word })}><WholeWord size={16} /></button>
        <button aria-pressed={options.regex} title={tr("Use regular expression", "Expresión regular")} aria-label={tr("Use regular expression", "Expresión regular")} onClick={() => update({ regex: !options.regex })}><Regex size={15} /></button>
        <button className="search-filter-toggle" aria-expanded={filters} title={tr("File filters", "Filtrar archivos")} aria-label={tr("File filters", "Filtrar archivos")} onClick={() => setFilters(!filters)}><ListFilter size={14} /></button>
      </div>
      {filters && <div className="repository-search-filters">
        <label>{tr("Files to include", "Incluir archivos")}<input placeholder="src/**, *.rs" value={options.include} maxLength={4096} onChange={e => update({ include: e.target.value })} /></label>
        <label>{tr("Files to exclude", "Excluir archivos")}<input placeholder="vendor/**, *.lock" value={options.exclude} maxLength={4096} onChange={e => update({ exclude: e.target.value })} /></label>
      </div>}
    </div>
    <div className="repository-search-summary" role="status">{loading ? <><LoaderCircle size={13} className="git-spinning" />{tr("Searching…", "Buscando…")}</>
      : result ? `${result.count}${result.limited ? "+" : ""} ${tr("results in", "resultados en")} ${result.files.length} ${tr("files", "archivos")}`
      : tr("Search tracked and unignored text files.", "Busca en archivos de texto no ignorados.")}</div>
    {sameQuery && search?.error && <p className="scm-feedback is-error" role="alert">{search.error}</p>}
    <div className="repository-search-results">
      {result?.files.slice(0, limit).map(file => <div key={file.path} className="repository-search-file">
        <button className="search-file-heading" title={file.path} aria-expanded={!collapsed.has(file.path)} onClick={() => setCollapsed(current => { const next = new Set(current); if (next.has(file.path)) next.delete(file.path); else next.add(file.path); return next; })}>
          {collapsed.has(file.path) ? <ChevronRight size={12} /> : <ChevronDown size={12} />}<FileCode2 size={13} /><strong>{file.path.split("/").pop()}</strong><small>{file.path.includes("/") ? file.path.slice(0, file.path.lastIndexOf("/")) : ""}</small><b>{file.matches.length}</b>
        </button>
        {!collapsed.has(file.path) && <FileResults file={file} root={root} request={search!.request_id} language={language} />}
      </div>)}
      {result && result.files.length > limit && <button className="git-load-more" onClick={() => setLimit(n => n + 200)}>{tr("Show more files", "Mostrar más archivos")}</button>}
      {result?.limited && <p className="git-hint">{tr("Results are limited. Refine the search or file filters.", "Resultados limitados. Ajusta la búsqueda o los filtros.")}</p>}
      {!!result?.skipped && <p className="git-hint">{result.skipped} {tr("binary, large or unreadable files skipped.", "archivos binarios, grandes o ilegibles omitidos.")}</p>}
    </div>
  </section>;
}
function FileResults({ file, root, request, language }: { file: FileMatches; root: string; request: string; language: "en" | "es" }) {
  const [limit, setLimit] = useState(100);
  return <>{file.matches.slice(0, limit).map((match, index) => <button key={`${match.line}:${match.column}:${index}`} className="search-match" title={`${file.path}:${match.line}:${match.column}\n${match.before}${match.matched}${match.after}`}
    onClick={() => postNative({ type: "open_search_match", root_path: root, request_id: request, path: file.path, line: match.line, column: match.column })}>
    <small>{match.line}</small><span>{match.before}<mark>{match.matched || "▏"}</mark>{match.after}</span>
  </button>)}{file.matches.length > limit && <button className="git-load-more" onClick={() => setLimit(n => n + 100)}>{language === "en" ? "Show more matches" : "Mostrar más coincidencias"}</button>}</>;
}
