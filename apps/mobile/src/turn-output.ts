const FILE_TOOLS = new Set(["Write", "Edit", "MultiEdit", "NotebookEdit"]);
const TEST_COMMANDS = /^(?:(?:cargo|go|swift|dotnet|npm|pnpm|yarn|bun|make)\s+test|cargo\s+nextest\s+run|(?:npx\s+)?(?:vitest|jest)|pytest|python3?\s+-m\s+pytest)(?:\s|$)/;

export function isFileEdit(tool: string): boolean {
  return FILE_TOOLS.has(tool);
}

/** Recognizes one direct test command. A combined script's status cannot identify which command ran or failed. */
export function testCommand(command: string): string | undefined {
  const trimmed = command.trim();
  const wrapped = /^(?:\/\S*\/)?(?:sh|bash|zsh|dash|ksh)\s+-l?c\s+(['"])([^\r\n]*)\1$/.exec(trimmed);
  const script = (wrapped?.[2] ?? trimmed).trim();
  if (/[;&|<>$\x60\r\n]/.test(script) || !TEST_COMMANDS.test(script)) return undefined;
  return script;
}

export function outputFacts(edits: number, tests: readonly { state: string }[]): Array<{ kind: "edits" | "tests"; label: string }> {
  const facts: Array<{ kind: "edits" | "tests"; label: string }> = [];
  if (edits) facts.push({ kind: "edits", label: `${edits} recorded file ${edits === 1 ? "edit" : "edits"}` });
  if (tests.length) {
    const failed = tests.filter(({ state }) => state === "failed").length;
    facts.push({
      kind: "tests",
      label: `${tests.length} test ${tests.length === 1 ? "command" : "commands"}${failed ? ` · ${failed} failed` : ""}`,
    });
  }
  return facts;
}
