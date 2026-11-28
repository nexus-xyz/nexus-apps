/**
 * Test support for the contract checks of the clients that must not build their own Gateway requests, or build them in
 * known places only (the pages sandbox host, the phone app, Nexus UI): find, in TypeScript and JavaScript source, every
 * place that sends a network request, hands the sender on, or writes a Gateway path. It reads the syntax tree, so a
 * comment, or a string that only names a request call, is not a site. No production code imports it. Keep a call's text
 * out of this file's comments: the desktop's `api-requirement.mjs` reads every file of `kit/src` for them and fails on
 * one it cannot balance.
 */
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";

/**
 * What a site is: a call that sends a request, a value that is the sender itself (`fetch-ref`: `const get = deps.fetch`
 * sends nothing here, and every call of `get` is one the scan cannot see), or a string that holds a Gateway path.
 */
export type SiteKind = "fetch" | "fetch-ref" | "xhr" | "socket" | "event-source" | "beacon" | "transfer" | "path";

export interface Site {
  kind: SiteKind;
  /** 1-based, in the source it was found in. */
  line: number;
}

/** `new X(…)` that opens a connection, by the class's name. */
const OPENED = new Map<string, SiteKind>([["XMLHttpRequest", "xhr"], ["WebSocket", "socket"], ["EventSource", "event-source"]]);
/** `x.f(…)` or `f(…)` that sends one, by the function's name (the global, `deps.fetch` and `deps["fetch"]` are the same site). */
const CALLED = new Map<string, SiteKind>([
  ["fetch", "fetch"],
  ["sendBeacon", "beacon"],
  // Expo's file system sends and receives files over HTTP.
  ["uploadAsync", "transfer"],
  ["downloadAsync", "transfer"],
  ["createDownloadResumable", "transfer"],
]);
/** The name of the sender a value reference to is a site of its own: anything but a call of it hands it on. */
const SENDER = "fetch";
const GATEWAY_PATH = /\/v1\//;

/** The files a scan reads: TypeScript and JavaScript, in every module flavour, and not tests or declarations. */
const SOURCE = /\.([cm]?[jt]s|[jt]sx)$/;
const NOT_SOURCE = /\.(test|d)\.([cm]?[jt]s|[jt]sx)$/;

const scriptKind = (file: string) => (/\.tsx$/.test(file) ? ts.ScriptKind.TSX : /\.jsx$/.test(file) ? ts.ScriptKind.JSX : /\.[cm]?js$/.test(file) ? ts.ScriptKind.JS : ts.ScriptKind.TS);

/** What an expression is once the syntax that does not change its value is taken off: `(0, f)`, `f!`, `f as T`. */
function bare(expression: ts.Expression): ts.Expression {
  let current = expression;
  for (;;) {
    if (ts.isParenthesizedExpression(current) || ts.isNonNullExpression(current) || ts.isAsExpression(current) || ts.isSatisfiesExpression(current) || ts.isTypeAssertionExpression(current)) {
      current = current.expression;
    } else if (ts.isBinaryExpression(current) && current.operatorToken.kind === ts.SyntaxKind.CommaToken) current = current.right;
    else return current;
  }
}

/** The name an expression is reached by: `f`, `x.f`, `x?.f`, `x["f"]`; undefined for any other. */
function nameOf(expression: ts.Expression): string | undefined {
  const found = bare(expression);
  if (ts.isIdentifier(found)) return found.text;
  if (ts.isPropertyAccessExpression(found)) return found.name.text;
  if (ts.isElementAccessExpression(found) && ts.isStringLiteralLike(found.argumentExpression)) return found.argumentExpression.text;
  return undefined;
}

/** Whether a node is what a call calls, through the syntax `bare` takes off: that call is its own site. */
function isCallee(node: ts.Node): boolean {
  let child = node;
  for (let parent = node.parent; parent; child = parent, parent = parent.parent) {
    if (ts.isParenthesizedExpression(parent) || ts.isNonNullExpression(parent) || ts.isAsExpression(parent) || ts.isSatisfiesExpression(parent) || ts.isTypeAssertionExpression(parent)) continue;
    if (ts.isBinaryExpression(parent) && parent.operatorToken.kind === ts.SyntaxKind.CommaToken && parent.right === child) continue;
    return ts.isCallExpression(parent) && parent.expression === child;
  }
  return false;
}

/**
 * Whether an identifier named as the sender is a use of its value: not a declaration, a member's name, a type or an
 * import. The expression of a heritage clause (`class A extends make(fetch)`) is a value, though the syntax tree holds it
 * in a type node: only `typeof x` (a type query) and a type's own name (a type reference) are types.
 */
function isValueUse(node: ts.Identifier): boolean {
  const parent = node.parent;
  if (
    ts.findAncestor(node, (ancestor) => ts.isTypeQueryNode(ancestor) || ts.isTypeReferenceNode(ancestor)) ||
    (ts.isTypeNode(parent) && !ts.isExpressionWithTypeArguments(parent)) ||
    ts.isQualifiedName(parent) ||
    ts.isImportOrExportSpecifier(parent) ||
    ts.isImportClause(parent) ||
    ts.isNamespaceImport(parent)
  ) {
    return false;
  }
  // `typeof fetch === "function"` asks about the value; it cannot send.
  if (ts.isTypeOfExpression(parent)) return false;
  // `x.fetch` is read at the access, as a whole.
  if (ts.isPropertyAccessExpression(parent) && parent.name === node) return false;
  // `const { fetch: get } = deps` takes the sender out of `deps`; `const { send = fetch } = opts` falls back to the global.
  if (ts.isBindingElement(parent)) return parent.initializer === node || (ts.isObjectBindingPattern(parent.parent) && (parent.propertyName ?? parent.name) === node);
  if (ts.isShorthandPropertyAssignment(parent)) return true;
  // A declaration's name (`const fetch`, a parameter, `{ fetch: impl }`, `<X fetch={f} />`, a member) is not a use.
  const named = parent as { name?: ts.Node };
  return named.name !== node;
}

/** Whether an import or export specifier names a type only (`import type { X }`, `import { type X }`), which carries no value. */
function isTypeOnlyBinding(specifier: ts.ImportSpecifier | ts.ExportSpecifier): boolean {
  if (specifier.isTypeOnly) return true;
  const owner = specifier.parent.parent;
  return (ts.isImportClause(owner) || ts.isExportDeclaration(owner)) && owner.isTypeOnly;
}

/** The sites of one source, in the order they appear. */
export function requestSites(source: string, file = "source.tsx"): Site[] {
  const tree = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, scriptKind(file));
  const sites: Site[] = [];
  const add = (node: ts.Node, kind: SiteKind) => sites.push({ kind, line: tree.getLineAndCharacterOfPosition(node.getStart(tree)).line + 1 });
  const visit = (node: ts.Node) => {
    if (ts.isCallExpression(node)) {
      const kind = CALLED.get(nameOf(node.expression) ?? "");
      if (kind) add(node, kind);
    } else if (ts.isNewExpression(node)) {
      const kind = OPENED.get(nameOf(node.expression) ?? "");
      if (kind) add(node, kind);
    } else if ((ts.isStringLiteralLike(node) || ts.isTemplateHead(node) || ts.isTemplateMiddle(node) || ts.isTemplateTail(node)) && GATEWAY_PATH.test(node.text)) {
      add(node, "path");
    } else if ((ts.isImportSpecifier(node) || ts.isExportSpecifier(node)) && node.propertyName?.text === SENDER && node.name.text !== SENDER && !isTypeOnlyBinding(node)) {
      // `import { fetch as get } from "undici"`: every later call is of `get`. Without the alias a call is of `fetch`, a site as it is.
      add(node, "fetch-ref");
    } else if (
      // A value that is the sender: `deps.fetch`, `deps["fetch"]`, `{ fetch }`, `const { fetch: send } = deps`, `fetch.call`, `x = fetch`.
      ((ts.isIdentifier(node) && node.text === SENDER && isValueUse(node)) ||
        (ts.isPropertyAccessExpression(node) && node.name.text === SENDER && !ts.isTypeOfExpression(node.parent)) ||
        (ts.isElementAccessExpression(node) && ts.isStringLiteralLike(node.argumentExpression) && node.argumentExpression.text === SENDER && !ts.isTypeOfExpression(node.parent))) &&
      !isCallee(node)
    ) {
      add(node, "fetch-ref");
    }
    ts.forEachChild(node, visit);
  };
  visit(tree);
  return sites;
}

/**
 * The sites of every TypeScript or JavaScript file under `directory` (tests and declaration files left out), by path
 * relative to it, and only the files that have one. Each file's kinds are in source order.
 */
export function requestSitesIn(directory: string): Record<string, SiteKind[]> {
  const found: Record<string, SiteKind[]> = {};
  const files = (readdirSync(directory, { recursive: true }) as string[]).filter((file) => SOURCE.test(file) && !NOT_SOURCE.test(file)).sort();
  for (const file of files) {
    const sites = requestSites(readFileSync(join(directory, file), "utf8"), file);
    if (sites.length > 0) found[file] = sites.map((site) => site.kind);
  }
  return found;
}
