import { CoreError, type OrgSummary } from "@nexus/kit";
import * as WebBrowser from "expo-web-browser";
import { useCallback, useEffect, useRef, useState } from "react";
import { accountDeletionFailure, deleteConfirmedAccount, deletionMoney, ownershipTransferUrl, previewAccountDeletion, type DeletionPreview } from "@/account-deletion";
import { useAuth, useCore } from "@/auth";
import { Busy, Field, Footnote, Loading, Page, Row, Section } from "@/list";

type Preview = { kind: "loading" } | DeletionPreview;

export default function DeleteAccount() {
  const core = useCore();
  const { signOut, ui, gateway } = useAuth();
  const [preview, setPreview] = useState<Preview>({ kind: "loading" });
  const [confirmation, setConfirmation] = useState("");
  const [problem, setProblem] = useState<string>();
  const [busy, setBusy] = useState(false);
  const deleting = useRef(false);
  const checkVersion = useRef(0);

  const check = useCallback(async () => {
    const version = ++checkVersion.current;
    setPreview({ kind: "loading" });
    setConfirmation("");
    setProblem(undefined);
    const next = await previewAccountDeletion(core);
    if (checkVersion.current === version) setPreview(next);
  }, [core]);

  useEffect(() => {
    void check();
    return () => { checkVersion.current += 1; };
  }, [check]);

  const transfer = async (org: OrgSummary) => {
    const url = ownershipTransferUrl({ ui, gateway, org });
    if (!url) return setProblem("The organization's Members page is unavailable. Try signing in again.");
    setProblem(undefined);
    try {
      await WebBrowser.openBrowserAsync(url);
    } catch {
      setProblem("The Members page could not be opened. Try again.");
    }
  };

  const remove = async () => {
    if (deleting.current || confirmation !== "DELETE" || preview.kind !== "ready" || preview.plan.signInNeeded) return;
    deleting.current = true;
    setBusy(true);
    setProblem(undefined);
    try {
      await deleteConfirmedAccount({ core, confirmation, signOut });
    } catch (error) {
      if (error instanceof CoreError && error.code === "owns_organization") await check();
      else setProblem(accountDeletionFailure(error));
    } finally {
      deleting.current = false;
      setBusy(false);
    }
  };

  if (preview.kind === "loading") return <Loading alone />;
  return (
    <Page onRefresh={() => { if (!deleting.current) void check(); }}>
      <Section footer="This permanently deletes your sign-in and closes your personal account. You leave every company organization you belong to. Billing and audit records remain under the privacy policy.">
        <Row label="Delete your Nexus account" tone="danger" />
      </Section>
      {problem ? <Footnote tone="danger">{problem}</Footnote> : null}
      {preview.kind === "failed" ? (
        <Section footer={preview.message} footerTone="danger">
          <Row label="Check again" center onPress={() => void check()} />
        </Section>
      ) : preview.kind === "ownership" ? (
        <>
          <Section title="Transfer ownership first" footer="Choose another member as owner on each organization's Members page. Invite someone first if you are its only member.">
            {preview.organizations.map((org) => (
              <Row key={org.id} label={org.name} detail="Transfer ownership on the web" chevron onPress={() => void transfer(org)} accessibilityRole="link" accessibilityHint="Opens the organization's Members page to choose a new owner" />
            ))}
          </Section>
          <Section footer="Return here after transferring every organization. We check ownership again before you can delete your account.">
            <Row label="Check ownership and continue" center onPress={() => void check()} />
          </Section>
        </>
      ) : (
        <>
          {preview.plan.left.length ? (
            <Section title="Organizations you leave">
              {preview.plan.left.map((org) => <Row key={org.id} label={org.name} />)}
            </Section>
          ) : null}
          {preview.plan.planEnded ? <Footnote>Your personal account's payment customer and saved payment methods are deleted too.</Footnote> : null}
          {deletionMoney(preview.plan).map((line) => <Footnote key={line}>{line}</Footnote>)}
          {preview.plan.signInNeeded ? (
            <Section footer="For your security, sign out and sign in again. Return here within ten minutes to delete your account.">
              <Row label="Sign out" onPress={signOut} />
            </Section>
          ) : (
            <>
              <Section title="Type DELETE to confirm">
                <Field value={confirmation} onChangeText={setConfirmation} placeholder="DELETE" autoCapitalize="characters" autoCorrect={false} editable={!busy} accessibilityLabel="Type DELETE to confirm account deletion" />
              </Section>
              <Section>
                <Row label={busy ? "Deleting…" : "Delete account permanently"} tone="danger" center disabled={busy || confirmation !== "DELETE"} accessory={busy ? <Busy /> : undefined} onPress={() => void remove()} />
              </Section>
            </>
          )}
        </>
      )}
    </Page>
  );
}
