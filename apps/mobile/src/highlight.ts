import { createHighlighterCoreSync, type HighlighterCore, type ThemedToken } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import bash from "shiki/langs/bash.mjs";
import css from "shiki/langs/css.mjs";
import diff from "shiki/langs/diff.mjs";
import go from "shiki/langs/go.mjs";
import html from "shiki/langs/html.mjs";
import javascript from "shiki/langs/javascript.mjs";
import json from "shiki/langs/json.mjs";
import kotlin from "shiki/langs/kotlin.mjs";
import python from "shiki/langs/python.mjs";
import rust from "shiki/langs/rust.mjs";
import sql from "shiki/langs/sql.mjs";
import swift from "shiki/langs/swift.mjs";
import tsx from "shiki/langs/tsx.mjs";
import typescript from "shiki/langs/typescript.mjs";
import yaml from "shiki/langs/yaml.mjs";
import githubDark from "shiki/themes/github-dark-default.mjs";
import githubLight from "shiki/themes/github-light.mjs";

// Nexus UI's code blocks (code-block.tsx): Shiki with github-light and github-dark-default, the same aliases and
// colour replacements, so code is coloured alike on the web and the phone. The JavaScript regex engine at ES2018,
// because Hermes runs no WebAssembly and lacks the newer RegExp flags; forgiving, because Hermes still rejects a few
// patterns in some grammars (Python's and Bash's left whole blocks plain), and this skips just those patterns.
// A fixed set of languages (about 1 MB of grammars); any other stays plain. Load more on demand if missed.
const LANGS = [bash, css, diff, go, html, javascript, json, kotlin, python, rust, sql, swift, tsx, typescript, yaml];
const ALIASES: Record<string, string> = { sh: "bash", shell: "bash", zsh: "bash", console: "bash", py: "python", ts: "typescript", js: "javascript", yml: "yaml", rs: "rust", golang: "go", jsx: "javascript" };
// Shiki's github-light at WCAG AA on a light code ground (Nexus UI's lib/code-colors.ts).
const REPLACEMENTS: Record<string, string> = { "#d73a49": "#cf222e", "#e36209": "#bc4c00", "#22863a": "#1a7f37" };

let highlighter: HighlighterCore | undefined;
// A language's first call also compiles its grammar's regexes, and Shiki counts that time against its per-line limit
// (500 ms). On a slow phone or CI runner the line ran out of time and came back as one plain token. So the first call
// runs with no limit (0), and later calls keep Shiki's.
const compiled = new Set<string>();

/** The code's lines as coloured tokens, or undefined for a language not loaded (or a grammar that fails). */
export function highlight(code: string, language: string | undefined, dark: boolean): ThemedToken[][] | undefined {
  if (!language) return undefined;
  const lang = ALIASES[language.toLowerCase()] ?? language.toLowerCase();
  try {
    highlighter ??= createHighlighterCoreSync({ engine: createJavaScriptRegexEngine({ target: "ES2018", forgiving: true }), langs: LANGS, themes: [githubLight, githubDark] });
    if (!highlighter.getLoadedLanguages().includes(lang)) return undefined;
    const theme = dark ? "github-dark-default" : "github-light";
    const tokens = highlighter.codeToTokensBase(code, { lang, theme, colorReplacements: REPLACEMENTS, ...(compiled.has(lang) ? {} : { tokenizeTimeLimit: 0 }) });
    compiled.add(lang);
    return tokens;
  } catch {
    return undefined;
  }
}
