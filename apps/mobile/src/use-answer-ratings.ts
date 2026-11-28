import { CoreError, type CoreClient, type SetThreadAnswerRating, type ThreadAnswerRatings } from "@nexus/kit";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AppState } from "react-native";
import { useAuth, useStored } from "./auth";
import { haptic } from "./haptics";
import { useToast } from "./toast";

type PendingRating = { answerSeq: number; value: SetThreadAnswerRating };
type Snapshot = {
  audience: ThreadAnswerRatings["audience"];
  ratings: ThreadAnswerRatings["ratings"];
  pending: PendingRating[];
};
type RatingOwner = { snapshot?: Snapshot; request?: AbortController };

export const answerRatingsOn = process.env.EXPO_PUBLIC_NEXUS_THREAD_ANSWER_RATINGS === "on";

export function useAnswerRatings(core: CoreClient, threadId: string, ownerId: string | undefined, firstAnswer: number | undefined) {
  const { gateway } = useAuth();
  const toast = useToast();
  const [viewer, setViewer] = useState<{ core: CoreClient; id?: string }>();
  useEffect(() => {
    if (!answerRatingsOn || !ownerId || viewer?.core === core) return;
    const controller = new AbortController();
    let checking = false;
    async function identify() {
      if (checking || AppState.currentState !== "active") return;
      checking = true;
      try {
        const { user } = await core.me(controller.signal);
        if (!controller.signal.aborted) setViewer({ core, id: user?.id });
      } catch { /* Try again on resume or while the transcript is open. */ }
      finally { checking = false; }
    }
    void identify();
    const timer = setInterval(() => void identify(), 10_000);
    const foreground = AppState.addEventListener("change", (state) => { if (state === "active") void identify(); });
    return () => { clearInterval(timer); foreground.remove(); controller.abort(); };
  }, [core, ownerId, viewer?.core]);

  const ownsThread = answerRatingsOn && !!ownerId && viewer?.core === core && viewer.id === ownerId;
  const storage = useStored<Snapshot>("draft", `answer-ratings:${encodeURIComponent(gateway ?? "")}:${ownerId ?? ""}:${threadId}`);
  const owner = useMemo<RatingOwner>(() => ({ snapshot: ownsThread ? storage.load() : undefined }), [ownsThread, core, storage]);
  const current = useRef(owner);
  current.current = owner;
  const [shown, setShown] = useState<{ owner: RatingOwner; snapshot?: Snapshot }>();
  const [failure, setFailure] = useState<{ owner: RatingOwner; answerSeq: number }>();
  const save = useCallback((snapshot?: Snapshot) => {
    if (current.current !== owner) return;
    owner.snapshot = snapshot;
    if (snapshot) storage.save(snapshot);
    else storage.remove();
    setShown({ owner, snapshot });
  }, [owner, storage]);

  const sync = useCallback(async () => {
    if (!ownsThread || firstAnswer === undefined || owner.request || current.current !== owner || AppState.currentState !== "active") return;
    const controller = new AbortController();
    owner.request = controller;
    const live = () => !controller.signal.aborted && current.current === owner;
    try {
      const first = await core.threadAnswerRatings(threadId, undefined, controller.signal);
      const ratings = first.ratings.slice();
      let before = first.nextBefore;
      while (live() && before !== null && before >= firstAnswer) {
        const page = await core.threadAnswerRatings(threadId, before, controller.signal);
        ratings.push(...page.ratings);
        before = page.nextBefore;
      }
      if (!live()) return;
      const pending = owner.snapshot?.pending ?? [];
      save({ audience: first.audience, ratings, pending });
      while (live()) {
        const next = owner.snapshot?.pending[0];
        if (!next) break;
        try {
          const saved = await core.setThreadAnswerRating(threadId, next.answerSeq, next.value, controller.signal);
          if (!live() || !owner.snapshot) break;
          save({
            ...owner.snapshot,
            ratings: [...owner.snapshot.ratings.filter((rating) => rating.answerSeq !== saved.answerSeq), saved],
            pending: owner.snapshot.pending.filter((item) => item !== next),
          });
          toast({
            text: saved.rating === "none" ? "Rating removed." :
              first.audience === "personal" ? "Rating saved privately." : "Rating saved for organization admins.",
            icon: "check", haptic: "success",
          });
        } catch (error) {
          if (!live() || !owner.snapshot) break;
          if (!(error instanceof CoreError) || error.status >= 500 || error.status === 429) break;
          save({ ...owner.snapshot, pending: owner.snapshot.pending.filter((item) => item !== next) });
          setFailure({ owner, answerSeq: next.answerSeq });
          haptic("error");
        }
      }
    } catch (error) {
      if (live() && error instanceof CoreError && [403, 404, 410].includes(error.status)) save();
    } finally {
      if (owner.request === controller) owner.request = undefined;
    }
  }, [core, firstAnswer, owner, ownsThread, save, threadId, toast]);

  useEffect(() => {
    if (!ownsThread || firstAnswer === undefined) return;
    void sync();
    const timer = setInterval(() => void sync(), 10_000);
    const foreground = AppState.addEventListener("change", (state) => { if (state === "active") void sync(); });
    return () => {
      clearInterval(timer);
      foreground.remove();
      owner.request?.abort();
      owner.request = undefined;
    };
  }, [firstAnswer, owner, ownsThread, sync]);

  const put = (answerSeq: number, value: Pick<SetThreadAnswerRating, "rating" | "reason" | "detail">) => {
    const snapshot = owner.snapshot;
    if (!snapshot || snapshot.pending.some((item) => item.answerSeq === answerSeq)) return;
    const version = snapshot.ratings.find((item) => item.answerSeq === answerSeq)?.version ?? 0;
    setFailure((failed) => failed?.answerSeq === answerSeq ? undefined : failed);
    save({
      ...snapshot,
      pending: [...snapshot.pending, { answerSeq, value: { ...value, audience: snapshot.audience, expectedVersion: version } }],
    });
    void sync();
  };
  const snapshot = shown?.owner === owner ? shown.snapshot : owner.snapshot;
  return { snapshot, put, failedAnswer: failure?.owner === owner ? failure.answerSeq : undefined };
}
