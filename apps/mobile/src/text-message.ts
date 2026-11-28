import * as SMS from "expo-sms";

/** Whether this phone can send a text: false on an iPad without Messages and in the iOS simulator. */
export const canText = (): Promise<boolean> => SMS.isAvailableAsync().catch(() => false);

/**
 * Opens the phone's Messages composer with `message` filled in. With no recipients, the person picks them in Messages,
 * so Nexus never reads the contacts. Nothing is sent until the person taps Send there. Android never reports the
 * outcome, so the result is `unknown` there.
 */
export async function composeText(message: string, to: string[] = []): Promise<SMS.SMSResponse["result"]> {
  return (await SMS.sendSMSAsync(to, message)).result;
}
