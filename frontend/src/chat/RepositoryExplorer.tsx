import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, FileCode2, FileJson, FileText, Folder, FolderOpen, GitBranch, RefreshCw, Search, X } from "lucide-react";
import type { ExplorerData, ExplorerRow, ChangeRow } from "./WorkspaceSurface";
import { postNative } from "../shared/native";

const rowHeight = 26;
const marker = { added: "A", deleted: "D", modified: "M", renamed: "R", untracked: "U", conflicted: "!" };
const fileIcon = (name: string) => /\.(json|ya?ml|toml)$/.test(name) ? FileJson : /\.(md|txt|log)$/.test(name) ? FileText : FileCode2;

export function RepositoryExplorer({ explorer, language, width, onResize }: {
  explorer: ExplorerData; language: "en" | "es"; width: number; onResize(value: number): void;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const [scroll, setScroll] = useState({ top: 0, height: 600 });
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const drag = useRef<{ x: number; width: number } | null>(null);
  const tr = (en: string, es: string) => language === "en" ? en : es;
  const files = explorer.mode === "files";
  const rows = useMemo(() => {
    const source: (ExplorerRow | ChangeRow)[] = files ? explorer.rows : explorer.changes;
    const term = query.trim().toLowerCase();
    return term ? source.filter(row => ("path" in row ? row.path : row.relative_path).toLowerCase().includes(term)) : source;
  }, [files, explorer.rows, explorer.changes, query]);
  useLayoutEffect(() => {
    const el = viewport.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setScroll({ top: el.scrollTop, height: el.clientHeight }));
    observer.observe(el);
    return () => observer.disconnect();
  }, [explorer.open]);
  useEffect(() => {
    setQuery(""); setActive(0);
    viewport.current?.scrollTo({ top: 0 });
  }, [explorer.root_path, explorer.mode]);
  useEffect(() => { setActive(index => Math.max(0, Math.min(index, rows.length - 1))); }, [rows.length]);
  if (!explorer.open) return null;
  const start = Math.max(0, Math.floor(scroll.top / rowHeight) - 10);
  const end = Math.min(rows.length, Math.ceil((scroll.top + scroll.height) / rowHeight) + 10);
  const activate = (row: ExplorerRow | ChangeRow, count = 1) => {
    if ("path" in row) {
      if (row.kind === "loading" || row.kind === "error") return;
      postNative({ type: "activate_file_row", path: row.path, kind: row.kind, click_count: count });
    } else postNative({ type: "open_repository_diff", relative_path: row.relative_path });
  };
  const focusRow = (index: number) => {
    const next = Math.max(0, Math.min(rows.length - 1, index));
    setActive(next);
    const el = viewport.current;
    if (el && next * rowHeight < el.scrollTop) el.scrollTop = next * rowHeight;
    else if (el && (next + 1) * rowHeight > el.scrollTop + el.clientHeight) el.scrollTop = (next + 1) * rowHeight - el.clientHeight;
  };
  return <aside className="workbench-explorer repository-explorer" style={{ width }}>
    <header><strong>{files ? tr("EXPLORER", "EXPLORADOR") : tr("SOURCE CONTROL", "CONTROL DE CÓDIGO")}</strong>
      <button title={tr("Refresh", "Actualizar")} aria-label={tr("Refresh", "Actualizar")} onClick={() => postNative({ type: "refresh_file_explorer" })}><RefreshCw size={14} /></button>
      <button title={tr("Close explorer", "Cerrar explorador")} aria-label={tr("Close explorer", "Cerrar explorador")} onClick={() => postNative({ type: "close_file_explorer" })}><X size={14} /></button>
    </header>
    <div className="repository-tabs" role="tablist" aria-label={tr("Repository views", "Vistas del repositorio")}>
      <button role="tab" aria-selected={files} onClick={() => postNative({ type: "set_file_explorer_mode", mode: "files" })}><FolderOpen size={14} />{tr("Files", "Archivos")}</button>
      <button role="tab" aria-selected={!files} onClick={() => postNative({ type: "set_file_explorer_mode", mode: "changes" })}><GitBranch size={14} />{tr("Changes", "Cambios")}<small>{explorer.changes.length}</small></button>
    </div>
    <div className="repository-filter"><Search size={13} /><input value={query} aria-label={tr("Filter loaded files", "Filtrar archivos cargados")}
      placeholder={tr("Filter loaded files…", "Filtrar archivos cargados…")} onChange={event => { setQuery(event.target.value); setActive(0); viewport.current?.scrollTo({ top: 0 }); }}
      onKeyDown={event => { if (event.key === "ArrowDown") { event.preventDefault(); viewport.current?.focus(); focusRow(0); } }} />
      {query && <button aria-label={tr("Clear filter", "Limpiar filtro")} onClick={() => setQuery("")}><X size={12} /></button>}
    </div>
    <div className="repository-root" title={explorer.root_path}><ChevronDown size={12} /><strong>{explorer.root_label}</strong></div>
    <div className="repository-tree" role="tree" tabIndex={0} aria-label={files ? tr("Files", "Archivos") : tr("Changes", "Cambios")}
      aria-activedescendant={rows[active] && active >= start && active < end ? `repository-row-${active}` : undefined}
      ref={viewport} onScroll={event => setScroll({ top: event.currentTarget.scrollTop, height: event.currentTarget.clientHeight })}
      onKeyDown={event => {
        const row = rows[active];
        if (!row) return;
        if (["ArrowDown", "ArrowUp", "PageDown", "PageUp", "Home", "End", "ArrowLeft", "ArrowRight", "Enter"].includes(event.key)) event.preventDefault();
        if (event.key === "ArrowDown") focusRow(active + 1);
        else if (event.key === "ArrowUp") focusRow(active - 1);
        else if (event.key === "PageDown") focusRow(active + Math.floor(scroll.height / rowHeight));
        else if (event.key === "PageUp") focusRow(active - Math.floor(scroll.height / rowHeight));
        else if (event.key === "Home") focusRow(0);
        else if (event.key === "End") focusRow(rows.length - 1);
        else if (event.key === "Enter") activate(row);
        else if ("path" in row && event.key === "ArrowRight" && row.kind === "directory") {
          if (!row.expanded) activate(row); else focusRow(active + 1);
        } else if ("path" in row && event.key === "ArrowLeft") {
          if (row.kind === "directory" && row.expanded) activate(row);
          else {
            for (let i = active - 1; i >= 0; i--) {
              const parent = rows[i];
              if ("depth" in parent && parent.depth < row.depth) { focusRow(i); break; }
            }
          }
        }
      }}>
      {!files && explorer.changes_state === "error" ? <div className="explorer-state is-error">{explorer.changes_error}</div>
        : !rows.length ? <div className="explorer-state">{!files && explorer.changes_state === "loading" ? tr("Loading changes…", "Cargando cambios…") : query ? tr("No matches in loaded files", "Sin coincidencias en los archivos cargados") : tr("No files to show", "No hay archivos para mostrar")}</div>
        : <div style={{ height: rows.length * rowHeight, position: "relative" }}>
          <div style={{ position: "absolute", top: start * rowHeight, left: 0, right: 0 }}>
            {rows.slice(start, end).map((row, offset) => {
              const index = start + offset;
              const isFile = "path" in row;
              const path = isFile ? row.path : row.relative_path;
              const label = isFile ? row.label : row.relative_path.split("/").pop()!;
              const directory = isFile && row.kind === "directory";
              const Icon = directory ? row.expanded ? FolderOpen : Folder : fileIcon(label);
              return <div role="treeitem" id={`repository-row-${index}`} key={path}
                aria-selected={row.selected} aria-level={isFile ? row.depth + 1 : 1} aria-expanded={directory ? row.expanded : undefined}
                title={path} className={`repository-row${row.selected ? " is-selected" : ""}${index === active ? " is-focused" : ""}${isFile && row.hidden ? " is-hidden-file" : ""}`}
                style={{ paddingLeft: 10 + (isFile ? row.depth * 14 : 0) }}
                onClick={event => { setActive(index); viewport.current?.focus(); activate(row, event.detail); }}>
                <span className="repository-chevron">{directory && (row.expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />)}</span>
                <Icon size={14} className={directory ? "folder-icon" : `file-icon ext-${label.split(".").pop()}`} />
                <span className="repository-filename">{label}</span>
                {!isFile && <><small>{row.relative_path.includes("/") ? row.relative_path.slice(0, row.relative_path.lastIndexOf("/")) : ""}</small><b className={`change-marker is-${row.kind}`}>{marker[row.kind]}</b></>}
              </div>;
            })}
          </div>
        </div>}
    </div>
    <footer className="repository-footer">{files ? tr("⌘P Quick open", "⌘P Apertura rápida") : `${explorer.changes.length} ${tr("changed files", "archivos modificados")}`}</footer>
    <div className="explorer-resize-handle" role="separator" tabIndex={0} aria-orientation="vertical" aria-label={tr("Resize explorer", "Cambiar ancho del explorador")} aria-valuenow={width} aria-valuemin={220} aria-valuemax={520}
      onKeyDown={event => { if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); const next = Math.max(220, Math.min(520, width + (event.key === "ArrowLeft" ? -10 : 10))); onResize(next); try { localStorage.setItem("blackholes-workbench-explorer-width", String(next)); } catch {} } }}
      onPointerDown={event => { if (event.button !== 0) return; event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); drag.current = { x: event.screenX, width }; }}
      onPointerMove={event => { if (drag.current) onResize(Math.max(220, Math.min(520, drag.current.width + event.screenX - drag.current.x))); }}
      onPointerUp={() => { drag.current = null; try { localStorage.setItem("blackholes-workbench-explorer-width", String(width)); } catch {} }}
      onPointerCancel={() => { drag.current = null; }} onLostPointerCapture={() => { drag.current = null; }} />
  </aside>;
}
