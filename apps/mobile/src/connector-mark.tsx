import Svg, { Path } from "react-native-svg";
import { CONNECTOR_MARKS } from "@/generated/connectors";
import { Icon } from "@/icon";
import { useTheme } from "@/theme";

/**
 * A connector's mark, as Nexus UI's `ConnectorMark` on Settings › Connectors: simple-icons' mark in its brand colour, or
 * in the foreground's when that colour is near black or white. One simple-icons leaves out (Slack, Teams) gets the
 * Connectors icon rather than a made-up mark.
 */
export function ConnectorMark({ name, size = 20 }: { name: string; size?: number }) {
  const theme = useTheme();
  const mark = CONNECTOR_MARKS[name];
  if (!mark) return <Icon name="connector" size={size} color={theme.muted} />;
  return (
    <Svg width={size} height={size} viewBox="0 0 24 24" accessibilityElementsHidden importantForAccessibility="no">
      <Path d={mark.path} fill={mark.hex ? `#${mark.hex}` : theme.foreground} />
    </Svg>
  );
}
