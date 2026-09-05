import { type PartialBlock } from "@blocknote/core";
import { filterSuggestionItems } from "@blocknote/core/extensions";
import { en, es } from "@blocknote/core/locales";
import { BlockNoteView } from "@blocknote/mantine";
import {
  getDefaultReactSlashMenuItems,
  SuggestionMenuController,
  useCreateBlockNote,
} from "@blocknote/react";
import { useEffect, useMemo, useRef } from "react";
import { postNative } from "../shared/native";
import "@blocknote/mantine/style.css";
import type { AppTheme } from "../shared/theme";

export type NoteBlocks = Record<string, unknown>[];

export interface NoteDocumentChange {
  content: string;
  blocks: NoteBlocks;
}

interface NotionNoteEditorProps {
  documentId: string;
  language: "en" | "es";
  markdown: string;
  blocks?: NoteBlocks | null;
  theme: AppTheme;
  onChange(change: NoteDocumentChange): void;
  onBlur(): void;
}

const allowedLink = /^(https?|mailto|tel):/i;
const invalidNavigationCharacter = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\uE000-\uF8FF\uFFFD]/;
const invalidNavigationCharacters = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\uE000-\uF8FF\uFFFD]/g;

type ArrowDirection = "up" | "down" | "left" | "right";

const privateArrowDirection = (key: string, keyCode = 0): ArrowDirection | null => ({
  "\uF700": "up" as const,
  "\uF701": "down" as const,
  "\uF702": "left" as const,
  "\uF703": "right" as const,
}[key] || ({ 37: "left" as const, 38: "up" as const, 39: "right" as const, 40: "down" as const })[keyCode] || null);

const stripNavigationCharacters = <T,>(value: T): T => {
  if (typeof value === "string") return value.replace(invalidNavigationCharacters, "") as T;
  if (Array.isArray(value)) return value.map(stripNavigationCharacters) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [key, stripNavigationCharacters(entry)]),
    ) as T;
  }
  return value;
};

const moveContentEditableSelection = (direction: ArrowDirection, extend: boolean) => {
  const selection = window.getSelection() as (Selection & {
    modify?: (alter: "move" | "extend", direction: string, granularity: string) => void;
  }) | null;
  if (!selection?.modify) return;
  const vertical = direction === "up" || direction === "down";
  selection.modify(
    extend ? "extend" : "move",
    direction === "up" ? "backward" : direction === "down" ? "forward" : direction,
    vertical ? "line" : "character",
  );
};

export function NotionNoteEditor({
  documentId,
  language,
  markdown,
  blocks,
  theme,
  onChange,
  onBlur,
}: NotionNoteEditorProps) {
  const hasStoredBlocks = Array.isArray(blocks) && blocks.length > 0;
  const hydrating = useRef(!hasStoredBlocks);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  const editor = useCreateBlockNote({
    initialContent: hasStoredBlocks
      ? (stripNavigationCharacters(blocks) as PartialBlock[])
      : [{ type: "paragraph" }],
    dictionary: {
      ...(language === "es" ? es : en),
      placeholders: {
        ...(language === "es" ? es.placeholders : en.placeholders),
        default: language === "es"
          ? "Escribe o usa ‘/’ para crear…"
          : "Write or use ‘/’ for commands…",
      },
    },
    links: {
      isValidLink: (href) => allowedLink.test(href),
      onClick: (event) => {
        const target = event.target;
        if (!(target instanceof Element)) return false;
        const href = target.closest("a")?.getAttribute("href");
        if (!href || !allowedLink.test(href)) return false;
        postNative({ type: "open_url", url: href });
        return true;
      },
    },
  }, [documentId, language]);

  useEffect(() => {
    hydrating.current = !hasStoredBlocks;
    if (!hasStoredBlocks) {
      const parsed = editor.tryParseMarkdownToBlocks(stripNavigationCharacters(markdown));
      editor.replaceBlocks(
        editor.document,
        parsed.length > 0 ? parsed : [{ type: "paragraph" }],
      );
    }
    queueMicrotask(() => { hydrating.current = false; });
  }, [documentId, editor, hasStoredBlocks, markdown]);

  const slashItems = useMemo(() => {
    return getDefaultReactSlashMenuItems(editor);
  }, [editor]);

  return (
    <div
      className="blackholes-notion-editor"
      onKeyDownCapture={(event) => {
        const direction = privateArrowDirection(event.key, event.keyCode || event.which || 0);
        if (!direction || !invalidNavigationCharacter.test(event.key)) return;
        event.preventDefault();
        event.stopPropagation();
        moveContentEditableSelection(direction, event.shiftKey);
      }}
      onBeforeInputCapture={(event) => {
        const data = (event.nativeEvent as InputEvent).data;
        if (typeof data === "string" && invalidNavigationCharacter.test(data)) event.preventDefault();
      }}
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) onBlur();
      }}
    >
      <BlockNoteView
        editor={editor}
        theme={theme}
        slashMenu={false}
        onChange={() => {
          if (hydrating.current) return;
          onChangeRef.current({
            content: stripNavigationCharacters(editor.blocksToMarkdownLossy(editor.document)),
            blocks: stripNavigationCharacters(editor.document) as unknown as NoteBlocks,
          });
        }}
      >
        <SuggestionMenuController
          triggerCharacter="/"
          getItems={async (query) => filterSuggestionItems(slashItems, query)}
        />
      </BlockNoteView>
    </div>
  );
}
