import { router } from "expo-router";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { initials } from "@/greeting";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { ACCESSIBILITY_FONT_SCALE } from "@/text-size";
import { font, text, useTheme } from "@/theme";

const AVATAR = 64;

/**
 * Who is signed in, at the top of Settings, as the chat apps open theirs: your initials on a large disc, your name and
 * the email you sign in with, centred. A tap opens Profile, so Settings needs no Profile row of its own.
 */
export function SettingsProfile({ name, email }: { name: string | null | undefined; email?: string }) {
  const theme = useTheme();
  const letters = initials(name);
  const title = name?.trim() || email || "Your profile";
  return (
    <Pressable
      onPress={() => {
        haptic("tap");
        router.push("/profile");
      }}
      style={styles.profile}
      accessibilityRole="button"
      accessibilityLabel={[title, name ? email : undefined].filter(Boolean).join(", ")}
      accessibilityHint="Opens your profile"
    >
      {({ pressed }) => (
        // A text button's press: it dims, the page stays.
        <View style={[styles.content, pressed && styles.pressed]}>
          <View style={[styles.avatar, { backgroundColor: theme.fill }]}>
            {letters ? (
              <Text style={[styles.letters, { color: theme.foreground }]} allowFontScaling={false}>
                {letters}
              </Text>
            ) : (
              <Icon name="person" size={28} color={theme.muted} />
            )}
          </View>
          <Text style={[styles.name, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
            {title}
          </Text>
          {name && email ? (
            <Text style={[styles.email, { color: theme.muted }]} numberOfLines={1} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
              {email}
            </Text>
          ) : null}
        </View>
      )}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  profile: { alignSelf: "center", maxWidth: "100%", paddingTop: 4, paddingBottom: 6 },
  content: { alignItems: "center" },
  pressed: { opacity: 0.55 },
  avatar: { width: AVATAR, height: AVATAR, borderRadius: AVATAR / 2, alignItems: "center", justifyContent: "center", marginBottom: 12 },
  letters: { fontFamily: font.medium, fontSize: 24, letterSpacing: 0.3 },
  name: { ...text.xl, fontFamily: font.medium, letterSpacing: -0.3, textAlign: "center" },
  email: { ...text.secondary, fontFamily: font.regular, textAlign: "center", marginTop: 2 },
});
