import Svg, { Path } from "react-native-svg";
import { MARK_LAYERS, MARK_VIEWBOX } from "@/nexus-mark-paths";
import { useTheme } from "@/theme";

/**
 * The Nexus mark, brand v5's F5 (Nexus AI 1), in one ink: the foreground's, on light and dark alike (the metal is for
 * marketing on black, never the product). Square, `size` points across; it keeps its own tilt and the half-width of
 * clear space around it is the caller's to leave. It only decorates: the words beside it say what it is.
 */
export function NexusMark({ size = 30, color }: { size?: number; color?: string }) {
  const theme = useTheme();
  const ink = color ?? theme.foreground;
  return (
    <Svg width={size} height={size} viewBox={MARK_VIEWBOX} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      {MARK_LAYERS.map((layer, n) => (
        <Path key={n} d={layer.d} fill="none" stroke={ink} strokeWidth={layer.width} strokeLinejoin="round" />
      ))}
    </Svg>
  );
}
