import { Stack } from "expo-router";
import { DrawerCard } from "@/drawer-card";

/** Conversation changes are instant; the drawer supplies the navigation's movement, with the chat as a card on iOS. */
export default function ChatLayout() {
  return (
    <DrawerCard>
      <Stack screenOptions={{ headerShown: false, animation: "none" }} />
    </DrawerCard>
  );
}
