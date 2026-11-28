import type { SessionEvent } from "@nexus/kit";
import type { ComputerSessionChanges } from "@nexus/kit/computer-types";

export interface TerminalCommand {
  id: string;
  command: string;
  result?: { output: string; failed: boolean; truncated: boolean };
}

export function terminalCommands(events: readonly SessionEvent[]): TerminalCommand[] {
  const commands = new Map<string, TerminalCommand>();
  for (const event of events) {
    if (event.kind === "tool" && event.name === "Bash" && event.input && typeof event.input === "object" && "command" in event.input && typeof event.input.command === "string") {
      commands.set(event.id, { id: event.id, command: event.input.command });
    } else if (event.kind === "toolResult") {
      const command = commands.get(event.id);
      if (command) command.result = {
        output: event.output.slice(-16_000),
        failed: event.isError ?? false,
        truncated: !!event.truncated || event.output.length > 16_000,
      };
    }
  }
  return [...commands.values()].slice(-24).reverse();
}

export function changedFiles(changes: ComputerSessionChanges) {
  const sections = changes.patch.split(/^diff --git /m).slice(1);
  return changes.files.map((path, index) => ({
    path,
    patch: sections[index] === undefined ? "" : `diff --git ${sections[index].replace(/\n$/, "")}`,
  }));
}
