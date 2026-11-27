import { CameraView, useCameraPermissions, type BarcodeSettings } from "expo-camera";
import * as Linking from "expo-linking";
import { useEffect, useRef, useState, type ComponentProps, type ReactNode } from "react";
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from "react-native";
import { haptic } from "./haptics";
import { Icon } from "./icon";
import { font, mono, text, useTheme } from "./theme";

type IconName = ComponentProps<typeof Icon>["name"];

export function ConnectHeader({ title, icon, children }: { title: string; icon?: IconName; children?: ReactNode }) {
  const theme = useTheme();
  return (
    <View style={styles.intro}>
      {icon && <View style={[styles.symbol, { backgroundColor: theme.card, borderColor: theme.border }]}
        accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
        <Icon name={icon} size={30} color={theme.foreground} />
      </View>}
      <Text accessibilityRole="header" style={[styles.title, { color: theme.foreground }]}>{title}</Text>
      {children && <ConnectMessage>{children}</ConnectMessage>}
    </View>
  );
}

export function ConnectButton({ label, onPress, icon, primary = false, disabled = false, busy = false }: {
  label: string;
  onPress: () => void;
  icon?: IconName;
  primary?: boolean;
  disabled?: boolean;
  busy?: boolean;
}) {
  const theme = useTheme();
  const unavailable = disabled || busy;
  const filled = primary && !disabled;
  const ink = filled ? theme.onPrimary : unavailable ? theme.muted : theme.foreground;
  return <Pressable
    accessibilityRole="button"
    accessibilityLabel={label}
    accessibilityState={{ disabled: unavailable, busy }}
    disabled={unavailable}
    onPress={() => {
      haptic("tap");
      onPress();
    }}
    style={({ pressed }) => [styles.button, {
      backgroundColor: filled ? theme.primary : pressed || unavailable ? theme.fill : theme.card,
      borderColor: filled ? theme.primary : theme.border,
      opacity: pressed && filled ? 0.8 : 1,
    }]}
  >
    {icon && <View style={styles.iconSlot} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <Icon name={icon} size={18} color={ink} />
    </View>}
    <Text style={[styles.action, { color: ink, opacity: busy ? 0 : 1 }]}>{label}</Text>
    {icon && <View style={styles.iconSlot} />}
    {busy && <ActivityIndicator style={StyleSheet.absoluteFill} color={ink} />}
  </Pressable>;
}

export function ConnectMessage({ children, error = false }: { children: ReactNode; error?: boolean }) {
  const theme = useTheme();
  return <Text accessibilityLiveRegion="polite" style={[styles.message, { color: error ? theme.danger : theme.muted }]}>{children}</Text>;
}

export function ConnectCode({ code }: { code: string }) {
  const theme = useTheme();
  return <View style={[styles.code, { backgroundColor: theme.fill }]}>
    <Text selectable accessibilityLabel={code.replace(/\s/g, "").split("").join(" ")} style={[styles.codeText, { color: theme.foreground }]}>{code}</Text>
  </View>;
}

const QR_ONLY: BarcodeSettings = { barcodeTypes: ["qr"] };

export function ConnectScanner({ active, onScan }: { active: boolean; onScan: (value: string) => void }) {
  const theme = useTheme();
  const [permission, requestPermission, getPermission] = useCameraPermissions();
  const [failed, setFailed] = useState(false);
  const [ready, setReady] = useState(false);
  const lastScan = useRef<string | undefined>(undefined);
  const blocked = permission?.granted === false && !permission.canAskAgain;
  useEffect(() => {
    if (!active) {
      setReady(false);
      return;
    }
    if (blocked) void getPermission().catch(() => setFailed(true));
  }, [active, blocked, getPermission]);

  if (failed) return <>
    <ConnectMessage error>The camera couldn’t start. Try again or use a connection link.</ConnectMessage>
    <ConnectButton label="Retry camera" icon="camera" primary onPress={() => { setReady(false); setFailed(false); }} />
  </>;

  if (!permission) return <ActivityIndicator color={theme.muted} />;
  if (!permission.granted) return <ConnectButton
    label={permission.canAskAgain ? "Enable camera" : "Open camera settings"}
    icon="camera"
    onPress={() => {
      const request = permission.canAskAgain ? requestPermission() : Linking.openSettings();
      void request.catch(() => setFailed(true));
    }}
    primary
  />;

  return <View style={[styles.camera, { backgroundColor: theme.fill }]}>
    {active && <CameraView
      style={StyleSheet.absoluteFill}
      facing="back"
      barcodeScannerSettings={QR_ONLY}
      onCameraReady={() => setReady(true)}
      onMountError={() => setFailed(true)}
      onBarcodeScanned={({ data }) => {
        if (lastScan.current === data) return;
        lastScan.current = data;
        onScan(data);
      }}
      accessibilityLabel="Computer connection QR scanner"
    />}
    {ready ? <View pointerEvents="none" style={styles.guide} /> : <ActivityIndicator color={theme.muted} />}
  </View>;
}

const styles = StyleSheet.create({
  intro: { alignItems: "center", gap: 8 },
  symbol: { width: 64, height: 64, borderRadius: 20, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth,
    alignItems: "center", justifyContent: "center", marginBottom: 4 },
  title: { ...text.largeTitle, fontFamily: font.medium, textAlign: "center" },
  button: { minHeight: 56, borderRadius: 28, borderWidth: StyleSheet.hairlineWidth, flexDirection: "row",
    alignSelf: "stretch", alignItems: "center", gap: 8, paddingHorizontal: 16, paddingVertical: 10 },
  iconSlot: { width: 20, alignItems: "center" },
  action: { ...text.base, flex: 1, fontFamily: font.medium, textAlign: "center" },
  message: { ...text.reading, fontFamily: font.regular, textAlign: "center", alignSelf: "center", maxWidth: 320 },
  code: { borderRadius: 16, paddingHorizontal: 16, paddingVertical: 20, alignItems: "center" },
  codeText: { ...text.xl, fontFamily: mono, textAlign: "center" },
  camera: { width: "100%", height: 224, overflow: "hidden", borderRadius: 24, alignItems: "center", justifyContent: "center" },
  guide: { width: 136, aspectRatio: 1, borderWidth: 2, borderColor: "#fff", borderRadius: 16, shadowColor: "#000", shadowOpacity: 0.7, shadowRadius: 2, shadowOffset: { width: 0, height: 0 } },
});
