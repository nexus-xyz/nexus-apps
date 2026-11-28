import { AGENT_ERRAND_ID } from "@nexus/kit";
import { useLocalSearchParams } from "expo-router";
import { View } from "react-native";
import { useAuth } from "@/auth";
import { ErrandThread } from "@/errand-thread";
import { Blank } from "@/list";

/** One errand as its own thread (A12), a page of the drawer beside the Assistant (`errand-thread.tsx`). */
export default function Errand() {
  const { agent, org } = useAuth();
  const { id } = useLocalSearchParams<{ id?: string }>();
  if (!agent || typeof id !== "string" || !AGENT_ERRAND_ID.test(id)) {
    return (
      <View style={{ flex: 1, justifyContent: "center" }}>
        <Blank title="This errand did not open" body={agent ? "The link does not name an errand." : "This build of the app is not connected to the agent service."} />
      </View>
    );
  }
  return <ErrandThread key={`${org}:${id}`} agent={agent} id={id} />;
}
