import type { Artifact, ArtifactAccess, ArtifactAudienceKind, MemberSummary, ShareChatSessionRequest, TeamSummary } from "@nexus/kit";
import * as Clipboard from "expo-clipboard";
import { useEffect, useState, type ComponentProps, type ReactNode } from "react";
import { ActivityIndicator, Pressable, ScrollView, Share, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import Reanimated, { FadeIn, FadeOut } from "react-native-reanimated";
import { useArtifacts, useAuth, useCore } from "@/auth";
import { TextInput } from "@/controls";
import { Icon } from "@/icon";
import { open } from "@/markdown";
import { DURATION } from "@/motion";
import { pressedFill, ripple } from "@/press";
import {
  absoluteLink,
  accessLine,
  addEmails,
  audienceChoices,
  audienceLabel,
  expiryChoices,
  inTreeOrder,
  personState,
  shareAs,
  shareButton,
  shareRequest,
  sharingSettings,
  whoHasAccess,
  type AudienceChoice,
  type Shareable,
} from "@/share";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { font, mono, text, useTheme } from "@/theme";

export type ShareActions = {
  /** What the sheet is about, at its head. */
  title: string;
  /** Who it is shared with now, under the title. */
  audience?: string;
  link?: string;
  /** Yours to share with a chosen audience, and to list who has access and revoke each. */
  shareable?: Shareable;
  /** A share made in the sheet (`shared`, once the sheet has closed), or one revoked there. */
  changed?: (shared?: Artifact) => void;
  stopSharing?: () => Promise<void>;
  continueChat?: () => Promise<void>;
  /** The Markdown to hand to the system's share sheet (Files, Mail, Notes…). */
  markdown?: () => Promise<string>;
  /** Nexus UI's page for it. */
  openUrl?: string;
  /** Yours to revoke: asked once more before it goes. */
  revoke?: () => Promise<void>;
};

type Step = "menu" | "revoke" | "busy" | "copied" | "audience" | "teams" | "people" | "external" | "public" | "access";
type IconName = ComponentProps<typeof Icon>["name"];

/** Each audience's mark, the same in "Share with" and "Who has access". */
const AUDIENCE_ICON: Record<ArtifactAudienceKind, IconName> = { org: "org", teams: "teams", people: "members", external: "mail", public: "link" };

/**
 * What the audiences offer, read when you choose one: the organization's settings, your role, its teams and members,
 * and its newest share (`current`), which "Share with" ticks and the pickers start from.
 */
type Offer = {
  choices: AudienceChoice[];
  expiries: { value: string; label: string }[];
  domains: string[];
  teams: TeamSummary[];
  members: MemberSummary[];
  orgName?: string;
  current?: Artifact;
};

/**
 * Sharing a chat or an artifact, one sheet: its steps (the menu, an audience and its picks, who has access, the revoke
 * check, "Copied") change in place, each with Back at its head. An audience the organization's settings forbid stays
 * listed, greyed, with the reason, as in Nexus UI. Sharing replaces what was shared before, so one audience stays open.
 */
export function ShareSheet({ actions, onClose, onError, solid }: { actions: ShareActions | null; onClose: () => void; onError: (message: string) => void; solid?: boolean }) {
  const core = useCore();
  const artifacts = useArtifacts();
  const { ui } = useAuth();
  const theme = useTheme();
  const { height } = useWindowDimensions();
  const [step, setStep] = useState<Step>("menu");
  const [copied, setCopied] = useState("Link copied");
  const [offer, setOffer] = useState<Offer | null>(null);
  const [teams, setTeams] = useState<string[]>([]);
  const [people, setPeople] = useState<string[]>([]);
  const [filter, setFilter] = useState("");
  // People outside: those added, the address being typed, and what was not one when it was added.
  const [outsiders, setOutsiders] = useState<string[]>([]);
  const [draft, setDraft] = useState("");
  const [rejected, setRejected] = useState<string[]>([]);
  const [expiry, setExpiry] = useState<string>();
  const [shares, setShares] = useState<Artifact[] | null>(null);
  const [access, setAccess] = useState<Record<string, ArtifactAccess>>({});
  const [confirming, setConfirming] = useState<string>();
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    setStep("menu");
    setOffer(null);
    setTeams([]);
    setPeople([]);
    setFilter("");
    setOutsiders([]);
    setDraft("");
    setRejected([]);
    setExpiry(undefined);
    setShares(null);
    setAccess({});
    setConfirming(undefined);
    setProblem(null);
  }, [actions]);
  if (!actions) return <Sheet visible={false} onClose={onClose} solid={solid}>{null}</Sheet>;
  const shareable = actions.shareable;

  // `presents`: the work opens a system sheet, which iOS drops while this one is still closing, so it waits it out.
  const run = (work: () => Promise<void>, presents = false) => {
    onClose();
    const go = () => work().catch((e: Error) => onError(e.message));
    if (presents) setTimeout(go, 450);
    else go();
  };
  const copy = async (link: string, said = "Link copied", then?: () => void) => {
    await Clipboard.setStringAsync(link);
    setCopied(said);
    setStep("copied");
    setTimeout(() => {
      onClose();
      then?.();
    }, 1100);
  };
  const fail = (e: Error) => {
    onClose();
    onError(e.message);
  };

  /** Each part may fail alone (an older Gateway, a list you may not read): the settings fall back to their defaults. */
  const choose = () => {
    setStep("audience");
    if (offer || !shareable) return;
    void Promise.allSettled([core.me(), core.orgSettings(), core.teams(), core.members(), whoHasAccess(artifacts, shareable)]).then(
      ([me, settings, teamList, memberList, active]) => {
        const who = me.status === "fulfilled" ? me.value : undefined;
        const policy = sharingSettings(settings.status === "fulfilled" ? settings.value : undefined);
        const mine = who?.user?.email?.toLowerCase();
        const personal = who?.tenant.kind === "personal";
        const orgName = personal ? undefined : who?.tenant.name;
        const current = active.status === "fulfilled" ? active.value[0] : undefined;
        // The pickers start from what it is shared with now, as Nexus UI's "More options" does.
        if (current?.audience.kind === "teams") setTeams(current.audience.teams);
        if (current?.audience.kind === "people") setPeople(current.audience.people);
        if (current?.audience.kind === "external") setOutsiders(current.audience.people);
        setOffer({
          choices: audienceChoices({ policy, role: who?.role, personal, page: shareable.kind === "page", orgName }),
          expiries: expiryChoices(policy["artifacts.external.expiry"]),
          domains: policy["artifacts.external.domains"].allow,
          teams: teamList.status === "fulfilled" ? inTreeOrder(teamList.value.teams) : [],
          members: memberList.status === "fulfilled" ? memberList.value.members.filter((m) => m.email && m.email.toLowerCase() !== mine) : [],
          orgName,
          current,
        });
      },
    );
  };

  const share = (request: ShareChatSessionRequest | undefined) => {
    if (!request || !shareable) return;
    setStep("busy");
    (async () => {
      const active = shareable.kind === "page" ? await whoHasAccess(artifacts, shareable) : [];
      const made = await shareAs(artifacts, shareable, request, active);
      const link = made.publicUrl ?? made.url;
      const whom = audienceLabel(made.audience).replace(/^\w/, (c) => c.toLowerCase());
      await copy(ui ? absoluteLink(link, ui) : link, `Shared with ${whom}. Link copied`, () => actions.changed?.(made));
    })().catch(fail);
  };

  const loadShares = () => {
    if (!shareable) return;
    whoHasAccess(artifacts, shareable).then(
      async (list) => {
        setShares(list);
        const outsideShares = list.filter((s) => s.audience.kind === "external" || s.audience.kind === "public");
        const read = await Promise.all(outsideShares.map((s) => artifacts.artifactAccess(s.id).then((a) => [s.id, a] as const, () => null)));
        setAccess(Object.fromEntries(read.filter((entry) => entry !== null)));
      },
      (e: Error) => setProblem(e.message),
    );
  };
  const revokeShare = (id: string) => {
    setConfirming(undefined);
    setProblem(null);
    artifacts.revokeArtifact(id).then(
      () => {
        setShares((list) => list?.filter((s) => s.id !== id) ?? null);
        actions.changed?.();
      },
      (e: Error) => setProblem(e.message),
    );
  };
  const changeAccess = (id: string, body: { grant?: string[]; deny?: string[]; remove?: string[] }) => {
    setProblem(null);
    artifacts.changeArtifactAccess(id, body).then(
      (next) => setAccess((all) => ({ ...all, [id]: next })),
      (e: Error) => setProblem(e.message),
    );
  };

  const list = { maxHeight: height * 0.45 };
  const waiting = <ActivityIndicator style={styles.loading} color={theme.muted} />;
  const picked = { teams, people, outside: outsiders, draft };
  // A team's or a member's name for the Share button: "Share with Platform", not its slug.
  const name = (value: string) =>
    offer?.teams.find((t) => t.slug === value)?.name ?? offer?.members.find((m) => m.email === value)?.name ?? value;
  const button = (kind: ArtifactAudienceKind, expiresIn?: string) => {
    const { label, ready } = shareButton(kind, picked, name);
    return <PrimaryButton label={label} ready={ready} onPress={() => share(shareRequest(kind, { ...picked, expiresIn }))} />;
  };

  const content = (() => {
    switch (step) {
      case "busy":
      case "copied":
        return <SheetTitle>{step === "busy" ? "Sharing…" : copied}</SheetTitle>;
      case "revoke":
        return (
          <>
            <SheetTitle>Revoke "{actions.title}"? Everyone it was shared with loses it; this can't be undone.</SheetTitle>
            <SheetRow label="Revoke" tone="danger" onPress={() => actions.revoke && run(actions.revoke)} />
            <SheetRow label="Cancel" tone="muted" onPress={onClose} />
          </>
        );
      case "audience": {
        const now = offer?.current;
        const where = offer?.orgName ?? "your organization";
        const group = (outsideOnes: boolean) =>
          offer?.choices
            .filter((choice) => choice.outside === outsideOnes)
            .map((choice) => (
              <AudienceRow
                key={choice.kind}
                choice={choice}
                current={now?.audience.kind === choice.kind}
                onPress={() => (choice.kind === "org" ? share({ audience: "org" }) : setStep(choice.kind))}
              />
            ));
        return (
          <>
            <StepHeader
              title="Share with"
              detail={now ? `Now: ${audienceLabel(now.audience)}` : offer ? "Now: only you" : undefined}
              onBack={() => setStep("menu")}
            />
            {offer ? (
              <>
                <GroupTitle>In {where}</GroupTitle>
                {group(false)}
                <GroupTitle>Outside {where}</GroupTitle>
                {group(true)}
                <Note>A new choice replaces the current one.{shareable?.kind === "chat" ? " Readers see the chat as it is now." : ""}</Note>
              </>
            ) : (
              waiting
            )}
          </>
        );
      }
      case "teams":
        return (
          <>
            <StepHeader title="Teams" detail="The teams under each one get it too." onBack={() => setStep("audience")} />
            <ScrollView style={list}>
              {offer?.teams.length ? (
                offer.teams.map((team) => (
                  <PickRow key={team.id} label={team.name} detail={team.slug} indent={team.depth - 1} picked={teams.includes(team.slug)} onPress={() => setTeams(toggle(teams, team.slug))} />
                ))
              ) : (
                <Note>Your organization has no teams yet.</Note>
              )}
            </ScrollView>
            {button("teams")}
          </>
        );
      case "people": {
        const needle = filter.trim().toLowerCase();
        const shown = (offer?.members ?? []).filter((m) => !needle || `${m.name ?? ""} ${m.email}`.toLowerCase().includes(needle));
        return (
          <>
            <StepHeader title="People" detail="Members of your organization." onBack={() => setStep("audience")} />
            <Field value={filter} onChangeText={setFilter} placeholder="Find by name or email" label="Find people" />
            <ScrollView style={list} keyboardShouldPersistTaps="handled">
              {shown.length ? (
                shown.map((m) => {
                  const email = m.email as string;
                  return <PickRow key={m.userId} label={m.name ?? email} detail={m.name ? email : undefined} picked={people.includes(email)} onPress={() => setPeople(toggle(people, email))} />;
                })
              ) : (
                <Note>{needle ? "Nobody matches." : "Nobody else is in your organization yet."}</Note>
              )}
            </ScrollView>
            {button("people")}
          </>
        );
      }
      case "external": {
        // A comma, a space or Return ends an address: it joins the list, and what is not one stays in the field.
        const add = (text: string) => {
          const { list: added, invalid } = addEmails(outsiders, text);
          setOutsiders(added);
          setDraft(invalid.join(", "));
          setRejected(invalid);
        };
        return (
          <>
            <StepHeader title="People outside" detail="They sign in with their address to open it." onBack={() => setStep("audience")} />
            <EmailField
              value={draft}
              onChangeText={(typed) => {
                setRejected([]);
                if (/[\s,;]$/.test(typed)) add(typed);
                else setDraft(typed);
              }}
              onAdd={() => add(draft)}
            />
            {rejected.length ? <Note danger>Not an email address: {rejected.join(", ")}</Note> : null}
            <ScrollView style={list} keyboardShouldPersistTaps="handled">
              {outsiders.map((email) => (
                <PersonRow key={email} email={email} onRemove={() => setOutsiders(outsiders.filter((e) => e !== email))} />
              ))}
            </ScrollView>
            <Note>
              Anyone else can only ask you for access. They see your name and organization, never the cost, your email or team names.
              {offer?.domains.length ? ` Allowed domains: ${offer.domains.join(", ")}.` : ""}
            </Note>
            {button("external")}
          </>
        );
      }
      case "public": {
        const chosen = expiry ?? offer?.expiries[0]?.value ?? "";
        return (
          <>
            <StepHeader title="Anyone with the link" detail="No sign-in needed. Choose when it expires." onBack={() => setStep("audience")} />
            {(offer?.expiries ?? []).map((choice) => (
              <PickRow key={choice.value} label={choice.value ? `In ${choice.label}` : "Never"} picked={chosen === choice.value} radio onPress={() => setExpiry(choice.value)} />
            ))}
            {button("public", chosen)}
          </>
        );
      }
      case "access":
        return (
          <>
            <StepHeader title="Who has access" detail={actions.title} onBack={() => setStep("menu")} />
            {problem ? <Note danger>{problem}</Note> : null}
            {shares === null ? (
              waiting
            ) : shares.length === 0 ? (
              <Note>Only you can open it.</Note>
            ) : (
              <ScrollView style={list}>
                {shares.map((s) => (
                  <ShareRow
                    key={s.id}
                    share={s}
                    access={access[s.id]}
                    confirming={confirming === s.id}
                    onRevoke={() => setConfirming(s.id)}
                    onConfirm={() => revokeShare(s.id)}
                    onKeep={() => setConfirming(undefined)}
                    onChange={(body) => changeAccess(s.id, body)}
                  />
                ))}
              </ScrollView>
            )}
          </>
        );
      case "menu":
        return (
          <>
            <SheetTitle>{actions.audience ? `${actions.title} · ${actions.audience}` : actions.title}</SheetTitle>
            {actions.continueChat ? <SheetRow label="Continue in a new chat" onPress={() => run(actions.continueChat!)} /> : null}
            {shareable ? <SheetRow label="Share with…" onPress={choose} /> : null}
            {actions.link ? <SheetRow label="Copy link" onPress={() => copy(actions.link!).catch((e: Error) => onError(e.message))} /> : null}
            {shareable ? (
              <SheetRow
                label="Who has access"
                onPress={() => {
                  setStep("access");
                  loadShares();
                }}
              />
            ) : null}
            {actions.markdown ? (
              <SheetRow
                label="Share as Markdown"
                onPress={() => run(async () => void (await Share.share({ title: actions.title, message: await actions.markdown!() })), true)}
              />
            ) : null}
            {actions.openUrl ? <SheetRow label="Open in Nexus UI" onPress={() => run(async () => open(actions.openUrl!), true)} /> : null}
            {actions.stopSharing ? <SheetRow label="Stop sharing" tone="danger" onPress={() => run(actions.stopSharing!)} /> : null}
            {actions.revoke ? <SheetRow label="Revoke" tone="danger" onPress={() => setStep("revoke")} /> : null}
            <SheetRow label="Cancel" tone="muted" onPress={onClose} />
          </>
        );
    }
  })();

  return (
    <Sheet visible onClose={onClose} solid={solid}>
      {content}
    </Sheet>
  );
}

const toggle = (list: string[], value: string) => (list.includes(value) ? list.filter((v) => v !== value) : [...list, value]);

/** A step's head: Back, its title, and a line on what it does. */
function StepHeader({ title, detail, onBack }: { title: string; detail?: string; onBack: () => void }) {
  const theme = useTheme();
  return (
    <View style={styles.header}>
      <Pressable onPress={onBack} hitSlop={5} style={[styles.back, { backgroundColor: theme.userBubble }]} accessibilityRole="button" accessibilityLabel="Back">
        <Icon name="back" size={15} color={theme.foreground} />
      </Pressable>
      <View style={styles.grow}>
        <Text style={[styles.heading, { color: theme.foreground }]} accessibilityRole="header" numberOfLines={1}>
          {title}
        </Text>
        {detail ? (
          <Text style={[styles.detail, { color: theme.muted }]} numberOfLines={2}>
            {detail}
          </Text>
        ) : null}
      </View>
    </View>
  );
}

function GroupTitle({ children }: { children: ReactNode }) {
  const theme = useTheme();
  return (
    <Text style={[styles.group, { color: theme.muted }]} accessibilityRole="header" numberOfLines={1}>
      {children}
    </Text>
  );
}

/** An audience: its mark, what it means (or, greyed with a lock, why not), a tick for the one shared now. */
function AudienceRow({ choice, current, onPress }: { choice: AudienceChoice; current: boolean; onPress: () => void }) {
  const theme = useTheme();
  const off = !!choice.refusal;
  return (
    <Pressable
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.row, pressed && !off && pressedFill(theme)]}
      onPress={onPress}
      disabled={off}
      accessibilityRole="button"
      accessibilityState={{ disabled: off, selected: current }}
    >
      <View style={[styles.tile, { backgroundColor: current ? theme.primaryFaint : theme.userBubble }, off && styles.faded]}>
        <Icon name={AUDIENCE_ICON[choice.kind]} size={17} color={current ? theme.primary : theme.foreground} />
      </View>
      <View style={styles.grow}>
        <Text style={[styles.label, { color: off ? theme.muted : theme.foreground }]}>{choice.label}</Text>
        <View style={styles.line}>
          {off ? <Icon name="lock" size={11} color={theme.muted} /> : null}
          <Text style={[styles.detail, styles.shrink, { color: theme.muted }]}>{choice.refusal ?? choice.detail}</Text>
        </View>
      </View>
      {current ? <Icon name="check" size={16} color={theme.primary} /> : null}
      {/* Every audience but the organization opens a step to choose in; the organization shares at once. */}
      {!off && choice.kind !== "org" ? <Icon name="forward" size={13} color={theme.tertiary} /> : null}
    </Pressable>
  );
}

/** A team, a member or an expiry to tick: a circle that fills when picked (`radio`: one of several). */
function PickRow({ label, detail, picked, indent = 0, radio, onPress }: { label: string; detail?: string; picked: boolean; indent?: number; radio?: boolean; onPress: () => void }) {
  const theme = useTheme();
  return (
    <Pressable
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.row, { paddingLeft: 20 + indent * 20 }, pressed && pressedFill(theme)]}
      onPress={onPress}
      accessibilityRole={radio ? "radio" : "checkbox"}
      accessibilityState={{ checked: picked }}
    >
      <Icon name={picked ? "picked" : "unpicked"} size={22} color={picked ? theme.primary : theme.tertiary} />
      <View style={styles.grow}>
        <Text style={[styles.label, { color: theme.foreground }]}>{label}</Text>
        {detail ? <Text style={[styles.detail, { color: theme.muted }]}>{detail}</Text> : null}
      </View>
    </Pressable>
  );
}

/** The step's one action, saying whom it shares with, or what is missing while it can't. */
function PrimaryButton({ label, ready, onPress }: { label: string; ready: boolean; onPress: () => void }) {
  const theme = useTheme();
  return (
    <Pressable
      style={({ pressed }) => [styles.button, { backgroundColor: ready ? theme.primary : theme.userBubble }, pressed && ready && styles.pressed]}
      onPress={onPress}
      disabled={!ready}
      accessibilityRole="button"
      accessibilityState={{ disabled: !ready }}
    >
      <Text style={[styles.buttonText, { color: ready ? theme.onPrimary : theme.muted }]} numberOfLines={1}>
        {label}
      </Text>
    </Pressable>
  );
}

/** One share in "Who has access": its audience, its Revoke (asked once, in place), and for one outside, each person. */
function ShareRow({
  share,
  access,
  confirming,
  onRevoke,
  onConfirm,
  onKeep,
  onChange,
}: {
  share: Artifact;
  access?: ArtifactAccess;
  confirming: boolean;
  onRevoke: () => void;
  onConfirm: () => void;
  onKeep: () => void;
  onChange: (body: { grant?: string[]; deny?: string[]; remove?: string[] }) => void;
}) {
  const theme = useTheme();
  const a = share.audience;
  // People outside are listed one by one below, once their access is read.
  const names = a.kind === "teams" ? a.teams : a.kind === "people" || (a.kind === "external" && !access) ? a.people : [];
  const detail = [
    `Version ${share.version}`,
    names.join(", "),
    share.expiresAt ? `expires ${new Date(share.expiresAt).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" })}` : "",
    access ? accessLine(access) : "",
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <View style={styles.share}>
      <View style={styles.head}>
        <View style={[styles.tile, { backgroundColor: theme.userBubble }]}>
          <Icon name={AUDIENCE_ICON[a.kind]} size={17} color={theme.foreground} />
        </View>
        <View style={styles.grow}>
          <Text style={[styles.label, { color: theme.foreground }]}>{audienceLabel(a)}</Text>
          <Text style={[styles.detail, { color: theme.muted }]}>{detail}</Text>
        </View>
        {confirming ? (
          <>
            <Small label="Revoke" tone="danger" onPress={onConfirm} />
            <Small label="Keep" onPress={onKeep} />
          </>
        ) : (
          <Small label="Revoke…" tone="danger" onPress={onRevoke} />
        )}
      </View>
      {access?.people
        .filter((p) => p.status !== "removed")
        .map((p) => (
          <View key={p.email} style={[styles.head, styles.person]}>
            <View style={styles.grow}>
              <Text style={[styles.email, { color: theme.foreground }]} numberOfLines={1}>
                {p.email}
              </Text>
              <Text style={[styles.detail, { color: theme.muted }]}>{personState(p)}</Text>
            </View>
            {p.status === "requested" ? (
              <>
                <Small label="Approve" tone="primary" onPress={() => onChange({ grant: [p.email] })} />
                <Small label="Deny" onPress={() => onChange({ deny: [p.email] })} />
              </>
            ) : p.status === "granted" ? (
              <Small label="Remove" tone="danger" onPress={() => onChange({ remove: [p.email] })} />
            ) : null}
          </View>
        ))}
    </View>
  );
}

function Small({ label, tone = "normal", onPress }: { label: string; tone?: "normal" | "danger" | "primary"; onPress: () => void }) {
  const theme = useTheme();
  const color = { normal: theme.foreground, danger: theme.danger, primary: theme.primary }[tone];
  return (
    <Pressable onPress={onPress} hitSlop={8} accessibilityRole="button" style={styles.small}>
      <Text style={[styles.smallLabel, { color }]}>{label}</Text>
    </Pressable>
  );
}

function Note({ children, danger }: { children: ReactNode; danger?: boolean }) {
  const theme = useTheme();
  return <Text style={[styles.note, { color: danger ? theme.danger : theme.muted }]}>{children}</Text>;
}

function Field({ value, onChangeText, placeholder, label }: { value: string; onChangeText: (text: string) => void; placeholder: string; label: string }) {
  const theme = useTheme();
  return (
    <TextInput
      style={[styles.input, { color: theme.foreground, backgroundColor: theme.userBubble }]}
      value={value}
      onChangeText={onChangeText}
      placeholder={placeholder}
      placeholderTextColor={theme.muted}
      autoCapitalize="none"
      autoCorrect={false}
      accessibilityLabel={label}
    />
  );
}

/** "Add an email": one address at a time, Add (or Return) puts it in the list and keeps the keyboard up for the next. */
function EmailField({ value, onChangeText, onAdd }: { value: string; onChangeText: (text: string) => void; onAdd: () => void }) {
  const theme = useTheme();
  return (
    <View style={[styles.field, { backgroundColor: theme.userBubble }]}>
      <TextInput
        style={[styles.fieldInput, { color: theme.foreground }]}
        value={value}
        onChangeText={onChangeText}
        placeholder="name@partner.com"
        placeholderTextColor={theme.muted}
        autoCapitalize="none"
        autoCorrect={false}
        keyboardType="email-address"
        textContentType="emailAddress"
        returnKeyType="done"
        submitBehavior="submit"
        onSubmitEditing={onAdd}
        accessibilityLabel="Add an email address"
      />
      {value.trim() ? (
        <Pressable onPress={onAdd} hitSlop={10} style={styles.add} accessibilityRole="button" accessibilityLabel="Add">
          <Text style={[styles.addLabel, { color: theme.primary }]}>Add</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

/** A person outside the share goes to: their initial, their address, and a way to take them off before sharing. */
function PersonRow({ email, onRemove }: { email: string; onRemove: () => void }) {
  const theme = useTheme();
  return (
    <Reanimated.View entering={FadeIn.duration(DURATION.fast)} exiting={FadeOut.duration(DURATION.exit)} style={styles.personRow}>
      <View style={[styles.avatar, { backgroundColor: theme.primaryFaint }]}>
        <Text style={[styles.initial, { color: theme.primary }]}>{email.charAt(0).toUpperCase()}</Text>
      </View>
      <Text style={[styles.label, styles.fill, { color: theme.foreground }]} numberOfLines={1} ellipsizeMode="middle">
        {email}
      </Text>
      <Pressable onPress={onRemove} hitSlop={9} style={[styles.remove, { backgroundColor: theme.userBubble }]} accessibilityRole="button" accessibilityLabel={`Remove ${email}`}>
        <Icon name="close" size={10} color={theme.muted} />
      </Pressable>
    </Reanimated.View>
  );
}

// The sheet's card is 28 rounded with 8 above and below its rows; the Share button sits 14 in from its edges, so its
// corners follow the card's (28 = 14 + 14).
const styles = StyleSheet.create({
  loading: { paddingVertical: 20 },
  header: { flexDirection: "row", alignItems: "center", gap: 12, paddingLeft: 14, paddingRight: 20, paddingTop: 6, paddingBottom: 6 },
  back: { width: 34, height: 34, borderRadius: 17, alignItems: "center", justifyContent: "center" },
  heading: { ...text.lg, fontFamily: font.medium },
  group: { ...text.xs, fontFamily: font.medium, paddingHorizontal: 20, paddingTop: 12, paddingBottom: 2 },
  row: { minHeight: 56, flexDirection: "row", alignItems: "center", gap: 12, paddingLeft: 20, paddingRight: 20, paddingVertical: 8 },
  tile: { width: 34, height: 34, borderRadius: 10, borderCurve: "continuous", alignItems: "center", justifyContent: "center" },
  faded: { opacity: 0.45 },
  grow: { flex: 1, gap: 2 },
  shrink: { flexShrink: 1 },
  line: { flexDirection: "row", alignItems: "center", gap: 6 },
  head: { flexDirection: "row", alignItems: "center", gap: 12 },
  label: { ...text.base, fontFamily: font.medium },
  detail: { ...text.xs, fontFamily: font.regular },
  email: { ...text.sm, fontFamily: mono },
  note: { ...text.xs, fontFamily: font.regular, paddingHorizontal: 20, paddingVertical: 8 },
  input: { ...text.base, fontFamily: font.regular, minHeight: 44, borderRadius: 12, paddingHorizontal: 12, marginHorizontal: 16, marginVertical: 4 },
  // The field sits 16 in from the card's edge: 12 + 16 = 28, the card's own radius.
  field: { flexDirection: "row", alignItems: "center", minHeight: 44, borderRadius: 12, borderCurve: "continuous", marginHorizontal: 16, marginVertical: 4, paddingLeft: 12, paddingRight: 14 },
  fieldInput: { ...text.base, fontFamily: font.regular, flex: 1, minHeight: 44 },
  add: { paddingVertical: 6, paddingLeft: 10 },
  addLabel: { ...text.base, fontFamily: font.medium },
  personRow: { flexDirection: "row", alignItems: "center", gap: 12, minHeight: 52, paddingHorizontal: 20 },
  avatar: { width: 32, height: 32, borderRadius: 16, alignItems: "center", justifyContent: "center" },
  initial: { ...text.sm, fontFamily: font.medium },
  fill: { flex: 1 },
  remove: { width: 26, height: 26, borderRadius: 13, alignItems: "center", justifyContent: "center" },
  button: { minHeight: 50, borderRadius: 14, borderCurve: "continuous", marginHorizontal: 14, marginTop: 8, marginBottom: 6, paddingHorizontal: 16, alignItems: "center", justifyContent: "center" },
  pressed: { transform: [{ scale: 0.96 }] },
  buttonText: { ...text.base, fontFamily: font.medium },
  share: { paddingHorizontal: 20, paddingVertical: 8, gap: 6 },
  person: { paddingLeft: 46 },
  small: { paddingVertical: 6 },
  smallLabel: { ...text.sm, fontFamily: font.medium },
});
