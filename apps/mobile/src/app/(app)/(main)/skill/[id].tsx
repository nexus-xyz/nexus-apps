import type { SkillDetail } from "@nexus/kit";
import { useLocalSearchParams } from "expo-router";
import { useEffect, useState } from "react";
import { StyleSheet, Text, View } from "react-native";
import { useArtifacts } from "@/auth";
import { Blank, Loading, Page, Row, Section } from "@/list";
import { font, mono, text, useTheme } from "@/theme";

const day = (iso: string) => new Date(iso).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" });

/** One skill, read-only (brand v5's P69a "A skill"): its command, what it is for, and its versions, newest first. */
export default function Skill() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const artifacts = useArtifacts();
  const theme = useTheme();
  const [skill, setSkill] = useState<SkillDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    artifacts.skill(id).then(setSkill, (e: Error) => setError(e.message));
  }, [artifacts, id]);
  if (error) return <Blank title="Couldn't load this skill" body={error} tone="danger" />;
  if (!skill) return <Loading alone />;
  return (
    <Page>
      <View style={styles.head} accessible accessibilityRole="header">
        <Text style={[styles.command, { color: theme.foreground }]}>/{skill.name}</Text>
        {skill.description ? <Text style={[styles.about, { color: theme.muted }]}>{skill.description}</Text> : null}
      </View>
      <Section title="Versions" footer="Edit it on a computer: a new version keeps the old ones.">
        {[...skill.versions]
          .sort((a, b) => b.version - a.version)
          .map((v) => (
            <Row key={v.version} label={`v${v.version}`} monoLabel detail={v.description !== skill.description ? v.description : undefined} value={day(v.createdAt)} />
          ))}
      </Section>
    </Page>
  );
}

const styles = StyleSheet.create({
  head: { paddingHorizontal: 4, gap: 6 },
  command: { fontFamily: mono, fontSize: 22, lineHeight: 28 },
  about: { ...text.secondary, fontFamily: font.regular },
});
