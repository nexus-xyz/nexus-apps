import {
  AlignmentType, Document, ExternalHyperlink, HeadingLevel, LevelFormat, Packer,
  Paragraph, Table, TableCell, TableRow, TextRun,
} from "docx";
import { Lexer, type Token, type Tokens } from "marked";

const LEVELS = [HeadingLevel.HEADING_1, HeadingLevel.HEADING_2, HeadingLevel.HEADING_3,
  HeadingLevel.HEADING_4, HeadingLevel.HEADING_5, HeadingLevel.HEADING_6];

type RunStyle = { bold?: boolean; italics?: boolean; strike?: boolean; font?: string };

function inline(tokens: readonly Token[], style: RunStyle = {}): (TextRun | ExternalHyperlink)[] {
  return tokens.flatMap((token): (TextRun | ExternalHyperlink)[] => {
    if (token.type === "br") return [new TextRun({ break: 1 })];
    if (token.type === "strong") return inline(token.tokens ?? [], { ...style, bold: true });
    if (token.type === "em") return inline(token.tokens ?? [], { ...style, italics: true });
    if (token.type === "del") return inline(token.tokens ?? [], { ...style, strike: true });
    if (token.type === "link") {
      const href: unknown = token.href;
      const children = inline(token.tokens ?? [], style);
      if (typeof href !== "string" || !/^https?:\/\//i.test(href)) return children;
      return [new ExternalHyperlink({ link: href, children })];
    }
    if (token.type === "codespan") return [new TextRun({ ...style, font: "Courier New", text: token.text })];
    if (token.type === "text" && token.tokens?.length) return inline(token.tokens ?? [], style);
    return "text" in token && typeof token.text === "string" ? [new TextRun({ ...style, text: token.text })] : [];
  });
}

function blocks(tokens: readonly Token[], depth = 0): (Paragraph | Table)[] {
  return tokens.flatMap((token): (Paragraph | Table)[] => {
    if (token.type === "space" || token.type === "def") return [];
    if (token.type === "heading") {
      return [new Paragraph({ heading: LEVELS[token.depth - 1], children: inline(token.tokens ?? []) })];
    }
    if (token.type === "blockquote") return blocks(token.tokens ?? [], depth);
    if (token.type === "code") {
      const source: string = token.text;
      return source.split("\n").map((text) => new Paragraph({ children: [new TextRun({ text, font: "Courier New" })] }));
    }
    if (token.type === "list") {
      const level = Math.min(8, depth);
      const items: Tokens.ListItem[] = token.items;
      return items.flatMap((item) => {
        const first = item.tokens[0];
        const content = first && "tokens" in first && first.tokens ? inline(first.tokens) : first ? inline([first]) : [];
        return [
          new Paragraph({
            children: content,
            ...(token.ordered ? { numbering: { reference: "ordered", level } } : { bullet: { level } }),
          }),
          ...blocks(item.tokens.slice(1), depth + 1),
        ];
      });
    }
    if (token.type === "table") {
      const row = (cells: Tokens.TableCell[], bold = false) => new TableRow({
        children: cells.map(({ tokens }) => new TableCell({ children: [new Paragraph({ children: inline(tokens, { bold }) })] })),
      });
      const rows: Tokens.TableCell[][] = token.rows;
      return [new Table({ rows: [row(token.header, true), ...rows.map((cells) => row(cells))] })];
    }
    const children = "tokens" in token && token.tokens ? inline(token.tokens ?? []) : inline([token]);
    return children.length ? [new Paragraph({ children })] : [];
  });
}

export async function wordDocument(title: string, markdown: string): Promise<Uint8Array<ArrayBuffer>> {
  const content = blocks(Lexer.lex(markdown));
  const file = new Document({
    title,
    numbering: {
      config: [{
        reference: "ordered",
        levels: Array.from({ length: 9 }, (_, level) => ({
          level, format: LevelFormat.DECIMAL, text: `%${level + 1}.`, alignment: AlignmentType.START,
          style: { paragraph: { indent: { left: (level + 1) * 360, hanging: 240 } } },
        })),
      }],
    },
    sections: [{ children: content.length ? content : [new Paragraph(title)] }],
  });
  return new Uint8Array(await Packer.toArrayBuffer(file));
}
