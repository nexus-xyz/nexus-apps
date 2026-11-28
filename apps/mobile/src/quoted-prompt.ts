export function formatQuotedPrompt(prompt: string, quotes: readonly string[]): string {
  const blocks = quotes.map((quote) => quote.replace(/\r\n/g, "\n").split("\n").map((line) => `> ${line}`).join("\n"));
  return [...blocks, prompt.trim()].filter(Boolean).join("\n\n");
}

export function readQuotedPrompt(content: string): { quotes: string[]; prompt: string } {
  const quotes: string[] = [];
  const blocks = content.split("\n\n");
  for (const block of blocks) {
    const lines = block.split("\n");
    if (!lines.every((line) => /^>(?: |$)/.test(line))) break;
    const quoted = lines.map((line) => line.replace(/^> ?/, "")).join("\n");
    if (!quoted.trim()) break;
    quotes.push(quoted);
  }
  return { quotes, prompt: blocks.slice(quotes.length).join("\n\n") };
}
