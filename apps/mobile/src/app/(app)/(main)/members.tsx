import type { CurrentTenantResponse, InvitationsResponse, MemberSummary, PendingPerson } from "@nexus/kit";
import { useCallback, useEffect, useState } from "react";
import { useCore } from "@/auth";
import { Empty, Footnote, Loading, Page, Row, Section, Segmented } from "@/list";
import { AdminHead, SheetField, SheetForm } from "@/people-controls";
import { memberLine, people } from "@/people-controls-words";
import { Sheet } from "@/sheet";
import { useToast } from "@/toast";

const ORDER = { owner: 0, admin: 1, member: 2 } as const;
const STATUS: Record<InvitationsResponse["results"][number]["status"], string> = {
  sent: "invitation sent",
  would_invite: "would be invited",
  already_member: "already a member",
  not_signed_in: "in the organization, not signed in yet",
  already_invited: "already invited",
  removed: "removed; an admin recovers them",
  failed: "failed",
};

/**
 * Members (brand v5's P71e·F on the P105·D system): the people who have not joined yet first, as one row that drills
 * in, then everyone with their role, each opening a person (`member/[id].tsx`). + invites someone by email in a sheet
 * (Gateway asks the auth provider to send it). Left out: the design's Service accounts, which the Gateway does not
 * have, and each person's home team in the list, which its member list does not carry. Changing a role, removing
 * someone and transferring ownership change the auth provider and Gateway together from Nexus UI's server, so they stay
 * on desktop and the web.
 */
export default function Members() {
  const core = useCore();
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [members, setMembers] = useState<MemberSummary[] | null>(null);
  const [pending, setPending] = useState<PendingPerson[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [inviting, setInviting] = useState(false);

  const manage = !!me?.permissions?.includes("members.manage");
  const load = useCallback(() => {
    core.me().then(setMe, () => undefined);
    core.members().then(
      (r) => {
        setMembers([...r.members].sort((a, b) => ORDER[a.role] - ORDER[b.role] || (a.name || a.email || "").localeCompare(b.name || b.email || "")));
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
  }, [core]);
  useEffect(load, [load]);
  useEffect(() => {
    if (manage) core.pendingPeople().then((r) => setPending(r.people), () => undefined);
  }, [core, manage, members]);

  const personal = me?.tenant.kind === "personal";
  const org = me?.tenant.name ?? "your organization";
  // Compared lower-cased on both sides: the member list and the team lists may not agree on case.
  const joined = new Set((members ?? []).map((m) => m.email?.toLowerCase()).filter(Boolean));
  const waiting = pending.filter((p) => !joined.has(p.email.toLowerCase()));

  return (
    <Page onRefresh={load}>
      <AdminHead
        title="Members"
        purpose={personal ? "Your personal organization: you are its one member." : `Who is in ${org}, and what each may do.`}
        add={manage && !personal ? { label: "Invite by email", onPress: () => setInviting(true) } : undefined}
      />
      {personal ? <Footnote>Create or join a company organization to invite people.</Footnote> : null}

      {manage && waiting.length ? (
        <Section>
          <Row label="Not joined yet" value={String(waiting.length)} href="/not-joined" />
        </Section>
      ) : null}

      <Section
        title={members ? people(members.length) : undefined}
        footer={manage && !personal ? "Change a role, remove someone or transfer ownership in Nexus for desktop or on the web." : undefined}
      >
        {error && !members ? <Empty tone="danger">Members could not be loaded. {error}</Empty> : null}
        {!members && !error ? <Loading bare /> : null}
        {members?.map((m) => (
          <Row
            key={m.userId}
            label={`${m.name || m.email || m.userId}${m.userId === me?.user?.id ? " (you)" : ""}`}
            detail={memberLine(m)}
            href={{ pathname: "/member/[id]", params: { id: m.userId } }}
          />
        ))}
      </Section>

      <Invite visible={inviting} onClose={() => setInviting(false)} onInvited={load} />
    </Page>
  );
}

/** Invite by email, as a member or an admin, in a sheet; they join when they accept. */
function Invite({ visible, onClose, onInvited }: { visible: boolean; onClose: () => void; onInvited: () => void }) {
  const core = useCore();
  const toast = useToast();
  const [email, setEmail] = useState("");
  const [admin, setAdmin] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const typed = email.trim().toLowerCase();

  const send = () => {
    if (!typed.includes("@")) return setError("Enter the person's email.");
    setBusy(true);
    setError(null);
    core
      .invite({ emails: [typed], role: admin ? "admin" : "member" })
      .then((r) => {
        const result = r.results[0];
        onInvited();
        if (!result || result.status === "failed") return setError(result ? `${result.email}: ${STATUS.failed}.` : "Nothing was sent.");
        toast({ text: `${result.email}: ${STATUS[result.status]}`, icon: "mail" });
        setEmail("");
        onClose();
      })
      .catch((e: Error) => setError(e.message))
      .finally(() => setBusy(false));
  };

  return (
    <Sheet visible={visible} onClose={onClose}>
      <SheetForm title="Invite by email" action="Send invitation" onSubmit={send} disabled={!typed} busy={busy} note="They join when they accept." error={error}>
        <SheetField
          value={email}
          onChangeText={setEmail}
          placeholder="name@company.com"
          autoCapitalize="none"
          autoCorrect={false}
          keyboardType="email-address"
          accessibilityLabel="Email address"
          returnKeyType="send"
          onSubmitEditing={send}
          autoFocus
        />
        <Segmented
          label="Role"
          options={[
            { id: "member", label: "Member" },
            { id: "admin", label: "Admin" },
          ]}
          value={admin ? "admin" : "member"}
          onChange={(v) => setAdmin(v === "admin")}
        />
      </SheetForm>
    </Sheet>
  );
}
