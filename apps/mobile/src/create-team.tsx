import { useState } from "react";
import { useCore } from "@/auth";
import { SheetField, SheetForm } from "@/people-controls";
import { Sheet } from "@/sheet";
import { SLUG, suggestSlug } from "@/teams";
import { useToast } from "@/toast";

/**
 * Create a team (owners and admins), at the top level or under `parent`, in a sheet (P105·D: editors as sheets): a
 * name, and the slug it suggests.
 */
export function CreateTeam({ visible, onClose, parent, onCreated }: { visible: boolean; onClose: () => void; parent?: { slug: string; name: string }; onCreated: () => void }) {
  const core = useCore();
  const toast = useToast();
  const [name, setName] = useState("");
  const [slug, setSlug] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const value = slug ?? suggestSlug(name);

  const create = () => {
    if (!SLUG.test(value)) return setError("The slug takes lower-case letters, digits and dashes, starting with a letter or digit.");
    setBusy(true);
    setError(null);
    core
      .createTeam({ slug: value, name: name.trim() || value, ...(parent ? { parent: parent.slug } : {}) })
      .then((team) => {
        toast({ text: `Created ${team.name}${parent ? ` under ${parent.name}` : ""}` });
        setName("");
        setSlug(undefined);
        onCreated();
        onClose();
      })
      .catch((e: Error) => setError(e.message))
      .finally(() => setBusy(false));
  };

  return (
    <Sheet visible={visible} onClose={onClose}>
      <SheetForm
        title={parent ? `A team under ${parent.name}` : "New team"}
        action={parent ? "Create child team" : "Create team"}
        onSubmit={create}
        disabled={!value}
        busy={busy}
        note="Open it to add people."
        error={error}
      >
        <SheetField value={name} onChangeText={setName} placeholder="Name, e.g. Platform" maxLength={80} accessibilityLabel="Team name" autoFocus />
        <SheetField value={value} onChangeText={setSlug} placeholder="Slug, e.g. platform" monoText autoCapitalize="none" autoCorrect={false} accessibilityLabel="Team slug" />
      </SheetForm>
    </Sheet>
  );
}
