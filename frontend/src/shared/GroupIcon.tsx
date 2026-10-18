import { Code2, Database, Folder, GitBranch, Globe2, Layers3, ListTodo, Rocket, SquareTerminal } from "lucide-react";

export const groupIcons = [
  { value: "layers", en: "Layers", es: "Capas", Icon: Layers3 },
  { value: "folder", en: "Folder", es: "Carpeta", Icon: Folder },
  { value: "code-2", en: "Code", es: "Código", Icon: Code2 },
  { value: "square-terminal", en: "Terminal", es: "Terminal", Icon: SquareTerminal },
  { value: "rocket", en: "Rocket", es: "Cohete", Icon: Rocket },
  { value: "database", en: "Database", es: "Base de datos", Icon: Database },
  { value: "globe", en: "Globe", es: "Globo", Icon: Globe2 },
  { value: "list-todo", en: "Checklist", es: "Checklist", Icon: ListTodo },
  { value: "git-branch", en: "Branch", es: "Rama", Icon: GitBranch },
] as const;

/** Icons are bundled artwork or escaped Unicode text, never user HTML. */
export function GroupIcon({ value, size = 20 }: { value: string; size?: number }) {
  if (value.startsWith("emoji:")) return <span className="group-emoji" style={{ fontSize: size, lineHeight: 1 }} aria-hidden="true">{value.slice(6)}</span>;
  const Icon = groupIcons.find(icon => icon.value === value)?.Icon || Layers3;
  return <Icon size={size} aria-hidden="true" />;
}
