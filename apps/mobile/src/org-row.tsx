import type { OrgSummary } from "@nexus/kit";
import { useState } from "react";
import { Image, StyleSheet, View } from "react-native";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { Busy, Row } from "@/list";
import { orgLabel } from "@/orgs";
import { useTheme } from "@/theme";

// The size of the settings rows' bare icons, so the organization's name starts where theirs do.
const LEAD = 20;

/**
 * The organization these settings belong to, first in its card of Settings: its logo where the rows have their icons
 * (the organization's own icon when it has none, the person in Personal), and its name. With more than one
 * organization, iOS's pop-up mark says a tap opens a choice, here the sheet of every organization (`org-sheet.tsx`); it
 * spins while a switch runs. Your role is on the sheet and on Members, not here: this row names the organization.
 */
export function OrgRow({ current, logo, count, switching, onOpen }: {
  current: OrgSummary | undefined;
  logo?: string;
  /** How many organizations you have, the one you are in included. */
  count: number;
  switching: boolean;
  onOpen: () => void;
}) {
  const theme = useTheme();
  const [logoFailed, setLogoFailed] = useState<string | null>(null);
  const name = current ? orgLabel(current) : "Organization";
  const canSwitch = count > 1;
  const lead =
    current?.kind !== "personal" && logo && logoFailed !== logo ? (
      // On a white tile, as the desktop sets it: a logo made for a white ground stays visible in dark.
      <View style={[styles.logoTile, { borderColor: theme.separator }]}>
        <Image source={{ uri: logo }} style={styles.logo} onError={() => setLogoFailed(logo)} accessibilityIgnoresInvertColors />
      </View>
    ) : (
      // No logo: the icon the rows under it would use, never a letter on a grey tile.
      <Icon name={current?.kind === "personal" ? "person" : "org"} size={LEAD} color={theme.muted} />
    );
  return (
    <Row
      bareIcon
      leading={lead}
      label={name}
      accessory={switching ? <Busy /> : canSwitch ? <Icon name="updown" size={14} color={theme.tertiary} /> : undefined}
      onPress={
        canSwitch
          ? () => {
              haptic("tap");
              onOpen();
            }
          : undefined
      }
      accessibilityHint={canSwitch ? `Switches organization, from ${count}. The settings below follow the one you choose.` : undefined}
      accessibilityState={{ busy: switching }}
    />
  );
}

const styles = StyleSheet.create({
  logoTile: { width: LEAD, height: LEAD, borderRadius: 5, borderCurve: "continuous", overflow: "hidden", backgroundColor: "#ffffff", borderWidth: StyleSheet.hairlineWidth },
  logo: { width: LEAD, height: LEAD },
});
