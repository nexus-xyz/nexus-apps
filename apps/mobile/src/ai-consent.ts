import { AUTO } from "./meta";
import { provider, providerOf } from "./providers";

/**
 * App Review 5.1.2(i): a chat message and its attachments go to the AI provider that serves the model, so the
 * chat asks once, before the first send, naming every provider the person's models can reach. A provider added later
 * asks again. The providers agreed to are kept on the device under `CONSENT` (`store.ts`) until the person withdraws
 * them in Settings or signs out.
 */
export const CONSENT = "consent:ai-providers:v2";

/** What the consent reads of a model: its host (`provider`, from the Rust gateway), else its maker. */
type Served = { id: string; provider?: string; owned_by?: string };

/** The providers the person agreed to, as `store.ts` keeps them. */
export interface Agreed {
  load(): string[] | undefined;
  save(providers: string[]): void;
}

/** The consent as the device stores it: `remove` throws when storage refuses. */
export interface Stored extends Agreed {
  remove(): void;
}

/**
 * The kept consent, with a withdrawal that holds even when storage refuses it. After `withdraw`, the chat asks again
 * until the next Allow, whatever the storage did. `withdraw` is true only once the stored Allow is gone, so Settings
 * can say when the change was not saved and could come back after the app restarts.
 */
export function withdrawable(stored: Stored): Agreed & { withdraw(): boolean } {
  let withdrawn = false;
  return {
    load: () => (withdrawn ? undefined : stored.load()),
    save: (providers) => {
      withdrawn = false;
      stored.save(providers);
    },
    withdraw: () => {
      withdrawn = true;
      try {
        stored.remove();
      } catch {
        return false;
      }
      return !stored.load()?.length;
    },
  };
}

const hostOf = (m: Served) => m.provider ?? m.owned_by ?? providerOf(m.id);

/**
 * The providers a chat message may reach: the host of each model the person may use (Auto picks among them), and the
 * chosen model's provider when the list lacks it (an older chat's model).
 */
export function providersFor(model: string, models: Served[]): string[] {
  const listed = models.filter((m) => m.id !== AUTO);
  const hosts = listed.map(hostOf);
  if (model !== AUTO && !listed.some((m) => m.id === model)) hosts.push(providerOf(model));
  return [...new Set(hosts)];
}

/**
 * Whether the person agrees to send to `providers`. If every one was agreed to before, it answers at once. Else `ask`
 * shows the question, and Allow keeps them all. A decline keeps nothing, so the next send asks again. With no provider
 * known (Auto before the model list loads), `ask` runs and nothing is kept.
 */
export async function agreeTo(providers: string[], agreed: Agreed, ask: () => Promise<boolean>): Promise<boolean> {
  const kept = agreed.load() ?? [];
  if (providers.length && providers.every((p) => kept.includes(p))) return true;
  if (!(await ask())) return false;
  if (providers.length) agreed.save([...new Set([...(agreed.load() ?? []), ...providers])]);
  return true;
}

/** A provider's name: the Gateway's (`GET /v1/providers`), else the one the app draws it with. */
export const providerName = (id: string, listed: { id: string; name: string }[]) => listed.find((p) => p.id === id)?.name ?? provider(id).name;

/** "A", "A or B", "A, B or C". */
const either = (names: string[]) => (names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} or ${names[names.length - 1]}`);

/** The question, naming every provider in `names`; with none known, the provider in general. */
export function consentText(names: string[], includesAttachments = true): { title: string; message: string } {
  const to = names.length ? `the AI provider whose model answers: ${either(names)}` : "the AI provider whose model answers";
  const data = includesAttachments ? "your messages and any photos or other files you attach" : "your messages";
  return {
    title: names.length === 1 ? `Send your chats to ${names[0]}?` : "Send your chats to AI providers?",
    message: `To answer, Nexus sends ${data} to ${to}.`,
  };
}
