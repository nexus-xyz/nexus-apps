import type { ModelEntry } from "@nexus/kit";
import { useEffect, useMemo, useState } from "react";
import { AUTO } from "./meta";
import { useStored, type useCore } from "./auth";

type Core = ReturnType<typeof useCore>;
/** `pricing`: what marks the models the free start pays for (`freeStartPays`); absent from a catalog cached by an older build. */
export type Model = Pick<ModelEntry, "id" | "name" | "owned_by" | "provider" | "default_for" | "context_window" | "pricing" | "input" | "reasoning" | "max_output_tokens" | "note">
  & Partial<Pick<ModelEntry, "allowed">>;

const AUTO_MODEL: Model = { id: AUTO, name: "Auto", owned_by: "nexus" };
// One list per signed-in client: the catalog barely changes, and every screen reads it.
const lists = new WeakMap<Core, Promise<Model[]>>();

/** The models you may chat with, Auto first, as Nexus UI's picker lists them. */
export function useModels(core: Core): Model[] {
  return useModelCatalog(core).models;
}

export function useModelCatalog(core: Core): { models: Model[]; catalog: Model[] } {
  // The catalog as it was last time, until the Gateway answers.
  const cache = useStored<Model[]>("cache", "models");
  const [catalog, setCatalog] = useState<Model[]>(() => cache.load() ?? [AUTO_MODEL]);
  useEffect(() => {
    let list = lists.get(core);
    if (!list) {
      list = core
        .models()
        .then((r) => [AUTO_MODEL, ...r.data.filter((m) => m.status === "supported" && m.surfaces.includes("chat") && m.id !== AUTO)])
        .catch(() => [AUTO_MODEL]);
      lists.set(core, list);
    }
    // A failed load (Auto alone) keeps the cached catalog.
    list.then((next) => {
      if (next.length > 1 || !cache.load()) setCatalog(next);
      if (next.length > 1) cache.save(next);
    });
  }, [core, cache]);
  const models = useMemo(() => catalog.filter((model) => model.allowed !== false), [catalog]);
  return { models, catalog };
}

/** A model's catalog name, else its id without the provider. */
export const nameOf = (models: Model[], id: string) => models.find((m) => m.id === id)?.name ?? id.slice(id.lastIndexOf("/") + 1);
