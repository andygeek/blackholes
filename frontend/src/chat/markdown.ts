const escapeHtml = (value: unknown): string => String(value).replace(/[&<>"']/g, (character) => ({
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
})[character] || character);

export const safeLink = (value: string): string | null => {
  try {
    const url = new URL(value);
    return ["http:", "https:"].includes(url.protocol) ? url.href : null;
  } catch {
    return null;
  }
};

const renderInlineMarkdown = (source: string, includeLinks = true): string => {
  // Render source spans directly instead of inserting control-character placeholders.
  // A link label can contain inline code without leaking a nested placeholder into HTML.
  const text = String(source || "");
  const pattern = /\x60([^\x60\n]+)\x60|\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)|<(https?:\/\/[^>\s]+)>|\*\*([^*\n]+)\*\*|__([^_\n]+)__|~~([^~\n]+)~~|(^|[\s([{>])\*([^*\n]+)\*(?=$|[\s)\]}.!,?:;])|(^|[\s([{>])_([^_\n]+)_(?=$|[\s)\]}.!,?:;])/g;
  let html = "";
  let cursor = 0;
  for (const match of text.matchAll(pattern)) {
    html += escapeHtml(text.slice(cursor, match.index));
    const [, code, label, href, autoLink, strong, strongUnderscore, deleted, italicPrefix, italic, underscorePrefix, italicUnderscore] = match;
    if (code !== undefined) {
      html += `<code class="markdown-inline-code">${escapeHtml(code)}</code>`;
    } else if (strong !== undefined || strongUnderscore !== undefined || deleted !== undefined) {
      const tag = deleted !== undefined ? "del" : "strong";
      html += `<${tag}>${renderInlineMarkdown(strong ?? strongUnderscore ?? deleted, includeLinks)}</${tag}>`;
    } else if (italic !== undefined || italicUnderscore !== undefined) {
      html += `${escapeHtml(italicPrefix ?? underscorePrefix ?? "")}<em>${renderInlineMarkdown(italic ?? italicUnderscore, includeLinks)}</em>`;
    } else {
      const safeHref = includeLinks ? safeLink(href || autoLink) : null;
      html += safeHref
        ? `<a class="markdown-link" href="${escapeHtml(safeHref)}">${label !== undefined ? renderInlineMarkdown(label, false) : escapeHtml(autoLink)}</a>`
        : escapeHtml(match[0]);
    }
    cursor = match.index + match[0].length;
  }
  return html + escapeHtml(text.slice(cursor));
};

const tableCells = (line: string): string[] => line
  .trim()
  .replace(/^\|/, "")
  .replace(/\|$/, "")
  .split("|")
  .map((cell) => cell.trim());

const tableDivider = (line: string): boolean => (
  /^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)+\|?\s*$/.test(line)
);

const listLine = (line: string) => line.match(/^(\s*)([-+*]|\d+[.)])\s+(.+)$/);
const fenceLine = (line: string) => line.match(/^\s*([\x60~]{3,})\s*([A-Za-z0-9_+#.-]*)?.*$/);

const startsMarkdownBlock = (lines: string[], index: number): boolean => {
  const line = lines[index] || "";
  return Boolean(
    fenceLine(line) ||
    /^(#{1,6})\s+/.test(line) ||
    /^\s*>\s?/.test(line) ||
    listLine(line) ||
    /^\s*((-{3,})|(\*{3,})|(_{3,}))\s*$/.test(line) ||
    (line.includes("|") && tableDivider(lines[index + 1] || "")),
  );
};

export const markdownToHtml = (markdown: string, language: "en" | "es"): string => {
  const lines = String(markdown || "").replace(/\r\n?/g, "\n").split("\n");
  const html: string[] = [];
  let index = 0;

  while (index < lines.length) {
    const line = lines[index];
    if (!line.trim()) {
      index += 1;
      continue;
    }

    const fence = fenceLine(line);
    if (fence) {
      const marker = fence[1];
      const markerCharacter = marker[0];
      const codeLanguage = fence[2] || "";
      const code: string[] = [];
      index += 1;
      while (index < lines.length) {
        const candidate = lines[index].trim();
        if (candidate.length >= marker.length && [...candidate].every((character) => character === markerCharacter)) {
          index += 1;
          break;
        }
        code.push(lines[index]);
        index += 1;
      }
      const genericLabel = language === "en" ? "Code" : "Código";
      const copyLabel = language === "en" ? "Copy" : "Copiar";
      html.push(
        '<div class="markdown-code">' +
          '<div class="markdown-code__header">' +
            `<span>${escapeHtml(codeLanguage || genericLabel)}</span>` +
            `<button class="markdown-code__copy" type="button">${copyLabel}</button>` +
          "</div>" +
          `<pre><code>${escapeHtml(code.join("\n"))}</code></pre>` +
        "</div>",
      );
      continue;
    }

    const heading = line.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      const level = heading[1].length;
      html.push(`<h${level}>${renderInlineMarkdown(heading[2])}</h${level}>`);
      index += 1;
      continue;
    }

    if (/^\s*((-{3,})|(\*{3,})|(_{3,}))\s*$/.test(line)) {
      html.push("<hr>");
      index += 1;
      continue;
    }

    if (line.includes("|") && tableDivider(lines[index + 1] || "")) {
      const headings = tableCells(line);
      const rows: string[][] = [];
      index += 2;
      while (index < lines.length && lines[index].includes("|") && lines[index].trim()) {
        rows.push(tableCells(lines[index]));
        index += 1;
      }
      html.push(
        '<div class="markdown-table-wrap"><table><thead><tr>' +
          headings.map((cell) => `<th>${renderInlineMarkdown(cell)}</th>`).join("") +
        "</tr></thead><tbody>" +
          rows.map((row) => `<tr>${headings.map((_, cellIndex) => `<td>${renderInlineMarkdown(row[cellIndex] || "")}</td>`).join("")}</tr>`).join("") +
        "</tbody></table></div>",
      );
      continue;
    }

    if (/^\s*>\s?/.test(line)) {
      const quote: string[] = [];
      while (index < lines.length && /^\s*>\s?/.test(lines[index])) {
        quote.push(lines[index].replace(/^\s*>\s?/, ""));
        index += 1;
      }
      html.push(`<blockquote>${quote.map((line) => renderInlineMarkdown(line)).join("<br>")}</blockquote>`);
      continue;
    }

    const firstListLine = listLine(line);
    if (firstListLine) {
      const ordered = /^\d/.test(firstListLine[2]);
      const start = ordered ? Number.parseInt(firstListLine[2], 10) || 1 : null;
      const items: string[] = [];
      while (index < lines.length) {
        const item = listLine(lines[index]);
        if (!item || /^\d/.test(item[2]) !== ordered) break;
        let content = item[3];
        index += 1;
        while (
          index < lines.length && lines[index].trim() && !listLine(lines[index]) &&
          !startsMarkdownBlock(lines, index) && /^\s{2,}/.test(lines[index])
        ) {
          content += `\n${lines[index].trim()}`;
          index += 1;
        }
        const checkbox = content.match(/^\[([ xX])\]\s+(.+)$/);
        if (checkbox) {
          items.push(
            '<li class="markdown-task-item">' +
              `<span class="markdown-checkbox${checkbox[1].toLowerCase() === "x" ? " is-checked" : ""}" aria-hidden="true"></span>` +
              `<span>${renderInlineMarkdown(checkbox[2])}</span>` +
            "</li>",
          );
        } else {
          items.push(`<li>${content.split("\n").map((line) => renderInlineMarkdown(line)).join("<br>")}</li>`);
        }
      }
      const tag = ordered ? "ol" : "ul";
      const startAttribute = ordered && start !== 1 ? ` start="${start}"` : "";
      html.push(`<${tag}${startAttribute}>${items.join("")}</${tag}>`);
      continue;
    }

    const paragraph = [line];
    index += 1;
    while (index < lines.length && lines[index].trim() && !startsMarkdownBlock(lines, index)) {
      paragraph.push(lines[index]);
      index += 1;
    }
    html.push(`<p>${paragraph.map((line) => renderInlineMarkdown(line)).join("<br>")}</p>`);
  }

  return html.join("");
};
