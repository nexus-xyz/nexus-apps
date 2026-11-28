import * as Clipboard from "expo-clipboard";
import { router, type Href } from "expo-router";
import * as WebBrowser from "expo-web-browser";
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { Image, Linking, Platform, Pressable, ScrollView, StyleSheet, Text, View, useColorScheme } from "react-native";
import { SvgXml } from "react-native-svg";
import { answerBlocks, type AnswerBlock as Block } from "@/answer-blocks";
import { diagramKey, diagramLabel, drawingOf, pendingDiagrams, requestDiagram, settleDiagram, subscribeDiagrams, type DiagramColors, type Drawing } from "@/diagrams";
import { highlight } from "@/highlight";
import { Icon } from "@/icon";
import { formula } from "@/math";
import MermaidRenderer from "@/mermaid-renderer";
import { parseMarkdownInline, type MarkdownInline as Inline, type MarkdownList } from "@nexus/kit";
import { pressedFill, ripple } from "@/press";
import { font, mono, text, useTheme, type Theme } from "@/theme";

/**
 * Where links go: `cite`, where the n-th source opens, if the text has one (a run page); `base`, a
 * repository's file URL that a relative file link (`src/index.ts`) opens under, as Nexus UI's `linkBase`; `resolve`,
 * where the page's own links go (the docs reader's), trusted to open where it says, else the rules for an answer;
 * `hosts`, show each web link's host beside its label, in an answer that read a connector.
 */
type Links = { cite?: (n: number) => string | undefined; base?: string; resolve?: (href: string) => string | undefined; hosts?: boolean };

/**
 * An answer as Markdown, read by @nexus/kit as Nexus UI reads it, drawn with T3's type (headings 21, 19, 17; code 13).
 * Math is drawn by MathJax (`math.ts`), and ```mermaid fences by the one renderer all diagrams share (`diagrams.ts`).
 */
export function Markdown({
  source,
  color,
  cite,
  linkBase,
  resolveLink,
  linkHosts,
}: {
  source: string;
  color?: string;
  cite?: Links["cite"];
  linkBase?: string;
  resolveLink?: Links["resolve"];
  linkHosts?: boolean;
}) {
  const theme = useTheme();
  const blocks = useMemo(() => answerBlocks(source), [source]);
  const links = { cite, base: linkBase, resolve: resolveLink, hosts: linkHosts };
  return (
    <View style={styles.prose}>
      {blocks.map((block, n) => (
        <BlockView key={n} block={block} theme={theme} color={color ?? theme.foreground} links={links} />
      ))}
    </View>
  );
}

function BlockView({ block, theme, color, links }: { block: Block; theme: Theme; color: string; links?: Links }) {
  const body = [styles.body, { color }];
  const spans = (source: string) => inlineSpans(source, theme, color, links);
  switch (block.kind) {
    case "heading":
      return (
        <Text selectable accessibilityRole="header" style={[body, styles.heading, HEADINGS[block.level - 1] ?? HEADINGS[3]]}>
          {spans(block.text)}
        </Text>
      );
    case "paragraph": {
      // An image alone in its paragraph (the usual way) is a card that loads it on a tap; one inside a sentence stays a link.
      const parts = parseMarkdownInline(block.text).filter((p) => !(p.kind === "text" && !p.text.trim()));
      const only = parts.length === 1 ? parts[0] : undefined;
      if (only?.kind === "image" && OPENS.test(only.src)) return <ImageCard src={only.src} alt={only.alt} theme={theme} />;
      return (
        <Text selectable style={body}>
          {spans(block.text)}
        </Text>
      );
    }
    case "quote":
      return (
        <View style={[styles.quote, { borderColor: theme.border }]}>
          <Text selectable style={[body, { color: theme.muted }]}>
            {spans(block.text)}
          </Text>
        </View>
      );
    case "list":
      return <ListView list={block} theme={theme} color={color} spans={spans} />;
    case "rule":
      return <View style={[styles.rule, { backgroundColor: theme.border }]} accessibilityRole="none" />;
    case "code":
      return <CodeBlock code={block.code} language={block.language} theme={theme} />;
    case "math":
      return <MathBlock tex={block.tex.trim()} color={color} theme={theme} />;
    case "diagram":
      return <Diagram source={block.source} theme={theme} />;
    case "table":
      return (
        // Fixed-width columns, scrolled sideways; measure the widest cell if tables look uneven.
        <ScrollView horizontal showsHorizontalScrollIndicator={false}>
          <View style={[styles.table, { borderColor: theme.border }]}>
            {[block.head, ...block.rows].map((row, r) => (
              <View key={r} style={[styles.tr, r > 0 && { borderTopWidth: StyleSheet.hairlineWidth, borderColor: theme.border }]}>
                {block.head.map((_, c) => (
                  <Text key={c} selectable style={[styles.cell, { color, textAlign: block.align[c] ?? "left" }, r === 0 && styles.th]}>
                    {spans(row[c] ?? "")}
                  </Text>
                ))}
              </View>
            ))}
          </View>
        </ScrollView>
      );
  }
}

/** A list, nested lists under their items; a task item shows its state with a box, ticked or not. */
function ListView({ list, theme, color, spans }: { list: MarkdownList; theme: Theme; color: string; spans: (source: string) => React.ReactNode }) {
  const body = [styles.body, { color }];
  return (
    <View style={styles.list}>
      {list.items.map((item, n) => (
        <View key={n}>
          <View style={styles.item}>
            {item.checked === undefined ? (
              <Text style={[body, styles.marker, { color: theme.muted }]}>{list.ordered ? `${list.start + n}.` : "•"}</Text>
            ) : (
              <View
                style={[styles.box, { borderColor: item.checked ? theme.primary : theme.border, backgroundColor: item.checked ? theme.primary : "transparent" }]}
                accessibilityLabel={item.checked ? "Done" : "Not done"}
              >
                {item.checked ? <Icon name="check" size={11} color={theme.onPrimary} /> : null}
              </View>
            )}
            <Text selectable style={[body, styles.itemText]}>
              {spans(item.text)}
            </Text>
          </View>
          {item.children ? (
            <View style={styles.nested}>
              <ListView list={item.children} theme={theme} color={color} spans={spans} />
            </View>
          ) : null}
        </View>
      ))}
    </View>
  );
}

/**
 * An image an answer shows: its alt text and host until someone taps it, then the picture (a tap opens it full size).
 * Never loaded on its own: a model can be led to write an image URL that carries the chat to another server.
 */
function ImageCard({ src, alt, theme }: { src: string; alt: string; theme: Theme }) {
  const [state, setState] = useState<"card" | "loading" | "shown" | "failed">("card");
  const [ratio, setRatio] = useState(4 / 3);
  const host = /^https?:\/\/([^/?#]+)/i.exec(src)?.[1] ?? src;
  if (state === "card" || state === "failed") {
    return (
      <Pressable
        android_ripple={ripple(theme)}
        style={({ pressed }) => [styles.imageCard, { borderColor: theme.border }, pressed && pressedFill(theme)]}
        onPress={() => setState("loading")}
        accessibilityRole="button"
        accessibilityLabel={`Load the image${alt ? ` ${alt}` : ""} from ${host}`}
      >
        <Text style={[styles.imageAlt, { color: theme.foreground }]}>{alt || "Image"}</Text>
        <Text style={[styles.imageHost, { color: state === "failed" ? theme.danger : theme.muted }]}>
          {state === "failed" ? `${host} · Couldn't load it. Tap to try again` : `${host} · Load image`}
        </Text>
      </Pressable>
    );
  }
  return (
    <Pressable onPress={() => open(src)} accessibilityRole="imagebutton" accessibilityLabel={alt || "Image"} accessibilityHint="Opens it full size">
      <Image
        source={{ uri: src }}
        style={[styles.image, { aspectRatio: ratio, backgroundColor: theme.userBubble }]}
        resizeMode="contain"
        onLoad={(e) => {
          const { width, height } = e.nativeEvent.source;
          if (width && height) setRatio(width / height);
          setState("shown");
        }}
        onError={() => setState("failed")}
      />
    </Pressable>
  );
}

/** Copies `text`; `copied` is true for a moment after. */
function useCopy(text: string) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await Clipboard.setStringAsync(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return [copied, copy] as const;
}

function CodeBlock({ code, language, theme }: { code: string; language?: string; theme: Theme }) {
  const [copied, copy] = useCopy(code);
  const dark = useColorScheme() === "dark";
  const lines = useMemo(() => highlight(code, language, dark), [code, language, dark]);
  return (
    <View style={[styles.code, { backgroundColor: theme.userBubble }]}>
      <View style={styles.codeHead}>
        <Text style={[styles.codeLang, { color: theme.muted }]}>{language ?? "code"}</Text>
        <Pressable accessibilityRole="button" style={styles.headButton} onPress={copy} accessibilityLabel={copied ? "Copied" : "Copy code"}>
          <Text style={[styles.codeLang, { color: copied ? theme.primary : theme.muted }]}>{copied ? "Copied" : "Copy"}</Text>
        </Pressable>
      </View>
      <ScrollView horizontal showsHorizontalScrollIndicator={false}>
        <Text selectable style={[styles.codeText, { color: theme.foreground }]}>
          {lines
            ? lines.map((line, n) => (
                <Text key={n}>
                  {line.map((token, t) => (
                    <Text key={t} style={{ color: token.color, fontStyle: (token.fontStyle ?? 0) & 1 ? "italic" : "normal" }}>
                      {token.content}
                    </Text>
                  ))}
                  {n < lines.length - 1 ? "\n" : ""}
                </Text>
              ))
            : code}
        </Text>
      </ScrollView>
    </View>
  );
}

// MathJax's em, in points: the answer's 16-point type at Nexus UI's KaTeX size (1.21em), so a formula's x-height meets
// the text's.
const MATH_EM = 16 * 1.21;

/** A display formula, centred, scrolling sideways when wider than the column; its TeX as code when MathJax can't draw it. */
function MathBlock({ tex, color, theme }: { tex: string; color: string; theme: Theme }) {
  const drawn = useMemo(() => formula(tex, true), [tex]);
  if (!drawn) return <CodeBlock code={tex} language="math" theme={theme} />;
  return (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.math}>
      <View accessible accessibilityRole="image" accessibilityLabel={tex}>
        <SvgXml xml={drawn.svg} width={drawn.width * MATH_EM} height={drawn.height * MATH_EM} color={color} />
      </View>
    </ScrollView>
  );
}

// Where a view in a line of text sits: on the baseline on Android. On iOS, React Native puts its bottom a 14-point
// system font's descent above the line's bottom, 2.6 points under the baseline of the answer's 16/23 DM Sans (measured
// on the simulator). note: one lift for every line; headings and table cells are within a point of it.
const INLINE_LIFT = Platform.OS === "ios" ? 2.6 : 0;

/** Inline math in its line, on the line's baseline; its TeX in code type when MathJax can't draw it. */
function InlineMath({ tex, color, theme }: { tex: string; color: string; theme: Theme }) {
  const drawn = useMemo(() => formula(tex, false), [tex]);
  if (!drawn) return <Text style={[styles.inlineCode, { backgroundColor: theme.userBubble }]}>{tex}</Text>;
  const width = drawn.width * MATH_EM;
  const height = drawn.height * MATH_EM;
  // Down by the formula's depth, so its baseline is the view's bottom; then up to the text's baseline.
  return (
    <View accessible accessibilityRole="image" accessibilityLabel={tex} style={{ width, height, transform: [{ translateY: drawn.depth * MATH_EM - INLINE_LIFT }] }}>
      <SvgXml xml={drawn.svg} width={width} height={height} color={color} />
    </View>
  );
}

/** A diagram's colours, from the app's palette. */
const diagramColors = (theme: Theme): DiagramColors => ({
  ground: theme.userBubble,
  node: theme.card,
  border: theme.tertiary,
  line: theme.muted,
  text: theme.foreground,
  muted: theme.muted,
});

/** The drawing of a diagram, queued for the shared renderer until it comes back. */
function useDrawing(source: string, theme: Theme): Drawing | undefined {
  const key = diagramKey(source, useColorScheme() === "dark");
  const drawing = useSyncExternalStore(subscribeDiagrams, () => drawingOf(key));
  useEffect(() => requestDiagram({ key, source, colors: diagramColors(theme) }), [key, source, theme]);
  return drawing;
}

/**
 * A ```mermaid fence as Nexus UI's diagram card: its kind, then Source and Copy, over the drawing. The drawing is as
 * wide as drawn, scaled down to fit the column but never below 72% (smaller text stops being readable, as on Nexus
 * UI), and past that it scrolls sideways. A diagram Mermaid can't read shows its source, with a note.
 */
function Diagram({ source, theme }: { source: string; theme: Theme }) {
  const drawing = useDrawing(source, theme);
  const [showSource, setShowSource] = useState(false);
  const [copied, copy] = useCopy(source);
  const [column, setColumn] = useState(0);
  const label = diagramLabel(source);
  if (drawing && "failed" in drawing) {
    return (
      <View style={styles.failed}>
        <CodeBlock code={source} language="mermaid" theme={theme} />
        {/* Nexus UI's `DIAGRAM_FAILED_NOTE`. */}
        <Text style={[styles.codeLang, { color: theme.muted }]}>Could not draw this diagram, so its source is shown instead.</Text>
      </View>
    );
  }
  const scale = drawing && column ? Math.min(1, Math.max(0.72, column / drawing.width)) : 1;
  return (
    <View style={[styles.code, { backgroundColor: theme.userBubble }]} onLayout={(e) => setColumn(e.nativeEvent.layout.width - 24)}>
      <View style={styles.codeHead}>
        <Text style={[styles.codeLang, { color: theme.muted }]}>{label}</Text>
        <View style={styles.actions}>
          <Pressable accessibilityRole="button" style={styles.headButton} onPress={() => setShowSource(!showSource)} accessibilityLabel={showSource ? "Show the diagram" : "Show the source"}>
            <Text style={[styles.codeLang, { color: theme.muted }]}>{showSource ? "Diagram" : "Source"}</Text>
          </Pressable>
          <Pressable accessibilityRole="button" style={styles.headButton} onPress={copy} accessibilityLabel={copied ? "Copied" : "Copy the diagram's source"}>
            <Text style={[styles.codeLang, { color: copied ? theme.primary : theme.muted }]}>{copied ? "Copied" : "Copy"}</Text>
          </Pressable>
        </View>
      </View>
      {showSource ? (
        <ScrollView horizontal showsHorizontalScrollIndicator={false}>
          <Text selectable style={[styles.codeText, { color: theme.foreground }]}>
            {source}
          </Text>
        </ScrollView>
      ) : drawing ? (
        <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.math}>
          <View accessible accessibilityRole="image" accessibilityLabel={`${label} diagram`}>
            <SvgXml xml={drawing.svg} width={drawing.width * scale} height={drawing.height * scale} />
          </View>
        </ScrollView>
      ) : (
        <Text style={[styles.codeLang, styles.drawing, { color: theme.muted }]}>Drawing the diagram…</Text>
      )}
    </View>
  );
}

// No progress from the renderer for this long (it could not load): the diagrams it still owes show their source. Long
// enough for Metro to bundle Mermaid the first time in development.
const RENDERER_TIMEOUT = 60_000;
// How long the renderer stays after its last drawing: for the next diagram of a streaming answer, and so the web view
// is still there when Expo sends it the reply to its last call.
const RENDERER_IDLE = 30_000;

const onDrawn = async (key: string, drawing: { svg: string; width: number; height: number } | null) => settleDiagram(key, drawing ?? { failed: true });
const RENDERER_PAGE = { style: { width: 320, height: 320 }, scrollEnabled: false };

/**
 * The one renderer every diagram is drawn by: a hidden web view (`mermaid-renderer.tsx`), mounted once, at the app's
 * root, while a diagram waits for it and for a short while after.
 */
export function DiagramRenderer() {
  const jobs = useSyncExternalStore(subscribeDiagrams, pendingDiagrams);
  const [awake, setAwake] = useState(false);
  useEffect(() => {
    if (!jobs.length) {
      const timer = setTimeout(() => setAwake(false), RENDERER_IDLE);
      return () => clearTimeout(timer);
    }
    setAwake(true);
    const timer = setTimeout(() => {
      for (const job of pendingDiagrams()) settleDiagram(job.key, { failed: true });
    }, RENDERER_TIMEOUT);
    return () => clearTimeout(timer);
  }, [jobs]);
  if (!jobs.length && !awake) return null;
  return (
    <View pointerEvents="none" style={styles.renderer}>
      <MermaidRenderer jobs={jobs} onDrawn={onDrawn} dom={RENDERER_PAGE} />
    </View>
  );
}

const OPENS = /^https?:\/\//i;

/** A relative path to a file (`src/index.ts`, `./README.md`), never a scheme (`javascript:`), as Nexus UI's. */
const RELATIVE_FILE = /^(?:\.\/)?(?![a-z][a-z0-9+.-]*:)(?!\/)[\w@.-][\w@./-]*(?:#[\w-]*)?$/i;

/**
 * Opens a link: one of the app's screens (`/run/…`) in the app, a web page in the in-app browser, as
 * Claude's and ChatGPT's apps do, anything else (mailto:) in its app. One that can't open is dropped rather than left
 * as an unhandled rejection.
 */
export function open(url: string) {
  if (/^\/(?!\/)/.test(url)) return router.push(url as Href);
  void (OPENS.test(url) ? WebBrowser.openBrowserAsync(url) : Linking.openURL(url)).catch(() => undefined);
}

/** The host a web link goes to. */
export function linkHost(href: string): string | undefined {
  if (!/^https?:\/\//i.test(href)) return undefined;
  try {
    // The URL's own host, never its user part: `https://jira.acme.example@evil.example` goes to evil.example.
    return new URL(href).hostname;
  } catch {
    return undefined;
  }
}

/** Inline Markdown as nested Text, so it wraps and selects as one paragraph. */
function inlineSpans(source: string, theme: Theme, color: string, links?: Links) {
  return parseMarkdownInline(source).map((part, n) => <Span key={n} part={part} theme={theme} color={color} links={links} />);
}

function Span({ part, theme, color, links }: { part: Inline; theme: Theme; color: string; links?: Links }) {
  switch (part.kind) {
    case "text":
      return part.text;
    case "code":
      return <Text style={[styles.inlineCode, { backgroundColor: theme.userBubble }]}>{part.text}</Text>;
    case "math":
      return <InlineMath tex={part.text} color={color} theme={theme} />;
    case "strong":
    case "em":
    case "strike":
      return (
        <Text style={part.kind === "strong" ? styles.strong : part.kind === "em" ? styles.em : [styles.strike, { color: theme.muted }]}>
          {part.children.map((child, n) => (
            <Span key={n} part={child} theme={theme} color={color} links={links} />
          ))}
        </Text>
      );
    case "cite": {
      const href = links?.cite?.(part.n);
      return href ? (
        <Text style={[styles.cite, { color: theme.primary }]} onPress={() => open(href)} accessibilityRole="link" accessibilityLabel={`Source ${part.n}`}>
          [{part.n}]
        </Text>
      ) : (
        `[${part.n}]`
      );
    }
    case "image": {
      // Never loaded on its own (a model can be led to write an image URL that sends the chat elsewhere): a link to tap.
      const label = `Image: ${part.alt || part.src}`;
      const host = links?.hosts && part.alt ? linkHost(part.src) : undefined;
      return OPENS.test(part.src) ? (
        <Text style={[styles.link, { color: theme.primary }]} onPress={() => open(part.src)} accessibilityRole="link">
          {label}
          {host ? <Text style={{ color: theme.muted }}>{` (${host})`}</Text> : null}
        </Text>
      ) : (
        label
      );
    }
    case "link": {
      // Only web links open; anything else (`javascript:`) stays its label. A relative file link opens under `base`.
      const resolved = links?.resolve?.(part.href);
      const href = resolved ?? (links?.base && RELATIVE_FILE.test(part.href) ? `${links.base}${part.href.replace(/^\.\//, "")}` : part.href);
      return resolved || OPENS.test(href) ? (
        <Text style={[styles.link, { color: theme.primary }]} onPress={() => open(href)} accessibilityRole="link">
          {/* The label is Markdown too: [`/llms.txt`](…) shows, and VoiceOver reads, no backticks. */}
          {inlineSpans(part.label, theme, color)}
          {links?.hosts && !resolved && linkHost(href) ? <Text style={{ color: theme.muted }}>{` (${linkHost(href)})`}</Text> : null}
        </Text>
      ) : (
        <Text>{inlineSpans(part.label, theme, color)}</Text>
      );
    }
  }
}

// Size, not weight, carries the hierarchy. Regular for the two larger levels, Medium for the two smaller, the smallest
// at the body's 17.
const HEADINGS = [
  { fontSize: 22, lineHeight: 28, fontFamily: font.display },
  { fontSize: 20, lineHeight: 26, fontFamily: font.regular },
  { fontSize: 18, lineHeight: 24, fontFamily: font.medium },
  { fontSize: 17, lineHeight: 24, fontFamily: font.medium },
];

const styles = StyleSheet.create({
  prose: { gap: 12 },
  body: { ...text.reading, fontFamily: font.regular },
  heading: { marginTop: 4 },
  quote: { borderLeftWidth: 3, paddingLeft: 12 },
  list: { gap: 6 },
  item: { flexDirection: "row", gap: 8 },
  marker: { minWidth: 16, fontVariant: ["tabular-nums"] },
  itemText: { flex: 1 },
  code: { borderRadius: 12, paddingHorizontal: 12, paddingBottom: 12 },
  // 44 pt high, so Copy and Source are full targets for TalkBack, VoiceOver and touch (hitSlop widens only touch).
  codeHead: { flexDirection: "row", justifyContent: "space-between", alignItems: "center", minHeight: 44 },
  headButton: { minWidth: 44, minHeight: 44, alignItems: "flex-end", justifyContent: "center" },
  codeLang: { ...text.xs, fontFamily: font.medium },
  codeText: { fontFamily: mono, fontSize: 13, lineHeight: 19 },
  inlineCode: { fontFamily: mono, fontSize: 15 },
  strong: { fontFamily: font.medium },
  em: { fontFamily: font.italic },
  link: { textDecorationLine: "underline" },
  strike: { textDecorationLine: "line-through" },
  rule: { height: StyleSheet.hairlineWidth, marginVertical: 6 },
  imageCard: { alignSelf: "flex-start", maxWidth: "100%", borderWidth: 1, borderRadius: 12, paddingHorizontal: 14, paddingVertical: 10, gap: 2 },
  imageAlt: { ...text.base, fontFamily: font.medium },
  imageHost: { ...text.xs, fontFamily: font.regular },
  image: { width: "100%", maxHeight: 360, borderRadius: 12 },
  nested: { paddingLeft: 22, marginTop: 6 },
  box: { width: 16, height: 16, borderRadius: 4, borderWidth: 1.5, alignItems: "center", justifyContent: "center", marginTop: 4 },
  cite: { fontFamily: font.medium, fontSize: 13 },
  table: { borderWidth: StyleSheet.hairlineWidth, borderRadius: 12, overflow: "hidden" },
  tr: { flexDirection: "row" },
  cell: { ...text.sm, fontFamily: font.regular, width: 140, paddingHorizontal: 10, paddingVertical: 8 },
  th: { fontFamily: font.medium },
  math: { flexGrow: 1, justifyContent: "center" },
  // Each button is 44 pt wide with its word at the right: a 2 pt gap leaves about 16 pt between the words.
  actions: { flexDirection: "row", gap: 2 },
  drawing: { paddingVertical: 24, textAlign: "center" },
  failed: { gap: 6 },
  // Out of sight and out of the way: drawn, so its page lays text out, but never seen or touched.
  renderer: { position: "absolute", top: 0, left: 0, width: 320, height: 320, opacity: 0 },
});
