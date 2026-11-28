import { Redirect, useLocalSearchParams } from "expo-router";
import { CodeControlPreview, previewScenario } from "@/code-control-preview";

export default function PreviewRoute() {
  const { scenario } = useLocalSearchParams<{ scenario?: string }>();
  if (!__DEV__) return <Redirect href="/sign-in" />;
  return <CodeControlPreview key={scenario ?? ""} initial={previewScenario(scenario)} />;
}
