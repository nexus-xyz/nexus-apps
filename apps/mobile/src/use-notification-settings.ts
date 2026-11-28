import type { NotificationGroup, NotificationSetting } from "@nexus/kit";
import { useFocusEffect } from "expo-router";
import { useCallback, useState } from "react";
import { useArtifacts, useStored } from "@/auth";

export type Channel = "email" | "push";

/**
 * What notifies you, where (brand v5's P97): read on each visit, kept as it was last time until the Gateway
 * answers, and changed one switch at a time. A change shows at once and is put back if the Gateway refuses it.
 */
export function useNotificationSettings() {
  const artifacts = useArtifacts();
  const cache = useStored<NotificationSetting[]>("cache", "notification-settings");
  const [settings, setSettings] = useState<NotificationSetting[] | null>(() => cache.load() ?? null);
  const [saving, setSaving] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const keep = useCallback(
    (next: NotificationSetting[]) => {
      setSettings(next);
      cache.save(next);
    },
    [cache],
  );
  useFocusEffect(
    useCallback(() => {
      artifacts.notificationSettings().then(keep, (e: Error) => setError(e.message));
    }, [artifacts, keep]),
  );

  const set = (group: NotificationGroup, channel: Channel, on: boolean) => {
    if (!settings || saving) return;
    const before = settings;
    setSettings(settings.map((s) => (s.group === group && s.channel === channel ? { ...s, setting: on ? "on" : "off" } : s)));
    setSaving(`${group}:${channel}`);
    setError(null);
    artifacts
      .setNotificationSettings([{ group, channel, setting: on ? "on" : "off" }])
      .then(keep, (e: Error) => {
        setSettings(before);
        setError(e.message);
      })
      .finally(() => setSaving(null));
  };

  const of = (group: NotificationGroup, channel: Channel) => settings?.find((s) => s.group === group && s.channel === channel);
  const pushAvailable = !!settings?.some((s) => s.channel === "push" && s.available);
  return { settings, saving, error, set, of, pushAvailable };
}
