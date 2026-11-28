# Nexus for iPhone, iPad and Android

One Expo (React Native) app on the organization's Gateway (ADR 0007, Nexus's design record). It
is the only phone app: there is no Swift app. Android has an internal channel ("Internal builds" below). iOS goes to
TestFlight ("TestFlight" below).

- **Gateway:** every call goes through `@nexus/kit` (`packages/kit`), the client Nexus UI and desktop use. It is not
  an npm dependency: `tsconfig.json` maps the import and `metro.config.js` watches the folder.
- **Shell:** a persistent chat and sidebar (`src/sidebar.tsx`) under the app's page navigator.
  On iOS, the gear shows Settings inside the drawer. Search settings finds pages by name and shows their section.
  Settings lists You, then the organization with your role. Members see People and Rules in the organization section.
  Close a settings page to return to the drawer. More holds Inbox, archived sessions and Help.
  Search and Customize have their own rows. Customize opens Integrations and Skills.
  The organization switcher opens as a sheet, with Personal first. Undo returns to the previous organization.
  Artifacts, Inbox and settings pages share a native sheet. Links within it use native push and Back.
  On Android, Settings opens as a page in the native stack.
  Privacy holds AI providers and crash reports. About holds support, the policies and the version.
  The chat's controls and composer float over the messages (`src/top-bar.tsx`), in Liquid Glass on supported iPhones.
  Swipe right across the chat to open the sidebar; vertical draft scrolling and text selection keep their native
  gestures. The drawer and page sheet respect Reduce Motion.
  The drawer groups sessions as the desktop's sidebar does: Projects, then Recent (`src/drawer-sessions.tsx`).
  A Code session goes under its project once the phone opens its title. Chats and locked sessions stay in Recent.
  Each row shows the desktop's mark: an ice ring while the agent works, and an amber dot while it waits on you.
  "Working" or "Needs you" then replaces the age. A folded project shows its loudest mark.
  The full Sessions list puts sessions that need a reply first, followed by running sessions and completed work by day.
  The phone reads "Needs you" from the session's latest event, so a sub-agent that runs on after a request reads as
  Working.

- **Android:** Material 3, not iOS's controls. The switch and the bottom sheet are Jetpack Compose's, from
  `@expo/ui`, in `*.android.tsx` files next to the iOS ones. The pickers are the app's own: a pill whose thumb slides
  (`src/segmented.android.tsx`), the mode switch's capsule, and menus with 16 dp corners and the chosen row filled. `src/material.tsx` sets every Material
  color role to a brand v4 metal, so no wallpaper color (Material You) reaches a control. Rows answer a touch with a
  ripple (`src/press.ts`). The sidebar is a modal navigation drawer with pill items. Headers keep the back arrow and
  a left title, and the chat's bar sets its title, or centers the mode switch. Compose must never reach the iOS bundle, so import `@expo/ui/jetpack-compose` only from an
  `.android.tsx` file or `src/material.tsx`.
  `app.json` blocks `SYSTEM_ALERT_WINDOW`, which Expo's template adds: the app never draws over other apps, and Google
  Play review asks about each permission. The photo picker needs no permission on Android.
- **Type:** brand v4's, set in `src/theme.ts`.
  Text is DM Sans Regular, with Medium for emphasis and buttons, and never bold. A title of 21 points or more takes
  DM Sans's 24 pt cut (`assets/fonts/`). Code, money and model ids are in Geist Mono.
- **Response actions**: touch and hold response text for native selection and copying. More under an
  answer offers Copy, Select text, Copy as Markdown, Share response, Read aloud and Report. Select text shows the
  whole answer in one selectable text view, so a selection can cross paragraphs, lists and code. Copy and Share
  response use the displayed words. Read aloud uses the device's speech engine and stops when the app leaves the
  foreground, the screen loses focus, or the person changes organizations or signs out.
- **Web sources:** a chat shows when it searches the web. Tap a site under the answer to see all sources.
  Tap a source in the list to open it in the browser.
- **The Assistant's name**: the Assistant goes by the name the person chose for their own agent profile,
  Nexus by default, in its bar, its profile header, the first open and the chat's labels (`src/use-assistant-name.ts`).
  "Sent using Nexus" and "Review in Nexus" name the product and stay. The approval card shows an email's subject, the
  body and its signature apart (A7·a), and a written result with a title gets its own row with Open (A7·f).
- **Voice mode** (Assistant A18 and A25): a build with `EXPO_PUBLIC_NEXUS_AGENT_VOICE=on` (set in `eas.json`) shows a voice
  button in the Assistant. Tap Talk to talk and tap again to send, or hold it while you talk. The words go as a normal
  ask, and Nexus says the answer. The Nexus mark shows the state (`src/voice-mark.tsx`), and `src/voice.ts` runs
  the turns. The phone's own recognizer makes the words (expo-speech-recognition), on the phone when it can. When it
  cannot, Apple's or the phone's speech service hears the audio, and the first press says so. iOS speaks in a solo
  ambient session, so the silent switch mutes it. Android speaks at the media volume. A tone picks one of A17's
  voices, as a pitch and a rate. Until the app reads the profile's tone, Calm's voice speaks. A build without the
  module shows no voice button.
- **Proactivity** (Assistant A10): Account › Assistant › Proactivity sets how often Nexus writes first.
  One dial: Quiet, Balanced or Hands-on, each with what it sends and about how often. Quiet hours hold everything but
  the urgent, from one typed time to another in the phone's time zone. Each change saves at once
  (`src/proactivity-words.ts`). An agent API with proactivity off says so on the page.
- **Errands** (Assistant A12): a build with `EXPO_PUBLIC_NEXUS_AGENT_ERRANDS=on` shows the errands that
  need you and those that run in a strip on top of the Assistant. When any errand waits on you, the chat says
  "2 errands need you" with a row per errand. A tap opens the errand as its own thread (
  `src/errand-thread.tsx`): its goal, steps and replies, with their approval cards. Reply about it in its composer.
  A draft has Confirm and Cancel. Long-press an errand in the strip or the drawer, or tap More in its thread, for
  Pin, Rename, Move to project, Mark done and Archive (`src/errand-menu.tsx`). Move to project (A14)
  lists the projects your errands are filed in, takes a new name, and moves a filed errand out; a filed errand says
  "in Fundraise". Errands in the Assistant's menu lists the open errands and the History, which a search narrows. An
  agent API with errands off hides them (`src/agent-errands.ts`).
- **Your accounts in Integrations** (Assistant A6): a build with `EXPO_PUBLIC_NEXUS_INTEGRATIONS=on`
  starts Integrations with your accounts. The title counts them and the ones that need you. Each service is one row,
  with its accounts inside, and says what Nexus can do with it, in Nexus UI's words. A connector's page says it too.
  Connecting stays on a computer.
- **Passwords and payments** (Assistant A8): Account › Assistant › Passwords and payments shows the
  linked password manager, the logins Nexus may ask to use with their grants, and the Nexus card with its limits per
  merchant. A long press on a login offers Change to: this once and Revoke now. A grant that waits opens its sheet:
  one login, one site, one task, Decline or Approve. With nothing linked, the page says so. The kit's readers refuse
  a body shaped like a secret, so the phone never holds or draws a password or a card number (`src/vault-words.ts`).
- **Guest mode**: a build with `EXPO_PUBLIC_NEXUS_GUEST=on` opens signed out on a guest Home, as
  Nexus UI's guest mode does. A guest chats a few rounds through Gateway's guest routes (ADR 0019), then the composer
  turns into a sign-in card. The app holds the chat, and a guest id stays in secure storage. Each round's Turnstile
  token comes from Nexus UI's challenge page in a small web view (`src/guest-challenge.tsx`, `src/guest.ts`). A
  computer's pairing link still opens sign-in.
- **Sign-in:** Apple, Google and email open the system sign-in sheet and use an authorization code with PKCE.
  On iOS 17.4 and later, the sheet returns directly to Nexus through its verified HTTPS callback on `nexus.inc`.
  iOS uses a private browser session so people can switch accounts after they sign out.
  Android and older iOS return through `xyz.nexus.app:/auth` after a confirmation in the browser.
  Android shares its browser session. To switch accounts, tap "Use another account" on the Continue page.
  Email opens the identity provider's email form, including codes and passwords for accounts that have one.
  The identity provider handles password recovery, device verification and MFA.
  Deploy Gateway and Nexus UI before releasing a mobile build that uses the new HTTPS callback.
  Nexus UI keeps the phone's request in Clerk's `choose-organization` task URL, which overrides custom navigation.
  Provider sign-in finishes on a page before a full browser load returns the code to the app; Next's router must
  not fetch a route handler that redirects to the app scheme. Other required authentication tasks stay with Clerk.
  The login screen uses Nexus's production Gateway and offers no alternate-server or device-code flow.
  The token is kept in the Keychain or the
  Android Keystore (`src/auth.tsx`); sign-out revokes it.
- **The Assistant's Privacy** (Assistant A11): Account › Assistant › Privacy pauses and resumes the
  Assistant after a confirm, revokes all access by name (the organization's accounts stay), and lists what Nexus can
  see per account: its reads, its writes after you approve, and what it never does. While paused, the Assistant
  shows "Paused at 14:02 · nothing runs until you resume" with Resume (`src/paused-bar.tsx`, `src/privacy-words.ts`).
- **The Assistant's Memory** (Assistant A16): Account › Assistant › Memory lists the memory files by kind and one per errand, each with its fact count. A file shows each fact with its source
  and day. Tap a fact for Show where, Edit and Delete. With the agent API's memory controls on, the page adds a note
  and pauses learning. Export shares a ZIP of the Markdown and `memory.json` on iOS, and the Markdown as text on
  Android. Forget all asks first. The files and the export are the kit's (`kit/src/agent-memory.ts`), as on the web.
- **Discover** (Assistant A15): the empty chat and Account › Assistant open Discover.
  Try these sends an idea to the Assistant. Connect more says what each account unlocks and opens its page.
  Autonomy shows the approval tiers with your spend line. Teach it about you adds a note and opens Memory. What's
  new has a dot until this phone shows it (`src/discover-words.ts`; the ideas and the ladder are the kit's).
- **The morning brief** (Assistant A1): today's thread starts with the brief from `GET /agent/brief/today`,
  with "Why am I seeing this?", Fewer like this and Turn off. It is off while a conversation from History is open
  (`src/brief-bubble.tsx`).
- **Managed configuration**: device management may pin the organization, with the key `org` (a slug or an
  id). After sign-in, the app switches to it; if the switch fails, it stays in the person's own.
  - iOS reads MDM AppConfig, the `UserDefaults` key `com.apple.configuration.managed`. Android reads its managed
    configuration (`RestrictionsManager`); `modules/managed-config/` declares the key for the EMM console.
  - Device management cannot pin the Gateway. New sign-ins use Nexus's production Gateway and do not show its address.
  - The app reads the key at start and each time it comes to the foreground (`src/managed.ts`, `src/managed-config.ts`).
  - Expo Go lacks the module, so nothing is managed there. On a simulator, a development build reads
    `xcrun simctl spawn <udid> defaults write xyz.nexus.ios com.apple.configuration.managed -dict org <slug>`.
    On an emulator, `adb shell` cannot set it: use a test device policy controller (Google's TestDPC).
- **Errors and crashes:** every uncaught JavaScript error and every native crash becomes a report with no
  content (`@nexus/kit`'s `diagnostics`, shared with the desktop app and Nexus UI). The app sends it to the signed-in
  Gateway, whose log carries it to the observability store (`src/reporting.tsx`, `modules/crash-reports/`).
  - iOS: MetricKit's crashes, with the crashed thread's frames.
  - Android: native crashes and ANRs from `ApplicationExitInfo` (Android 11 and later), and Java crashes
    from an uncaught-exception handler, as a name and a message with no frames. The first start of a build with the
    module reports no crash from before it. A fatal JavaScript error is reported once, by the JavaScript side.
  - To test it on a store or TestFlight build, open `nexus://diagnostics/test-crash?kind=js` or `?kind=native`
    from Notes, Safari or Chrome. The app asks first, then crashes. Open the app again to send the report.
    On Android, `native` sends the app SIGABRT, which Android records as a native crash.
  - The JavaScript error's message is `Forced test error`. On iOS, a fatal JavaScript error in a release
    build is also reported by MetricKit as a crash.
- **Shared links**: the app claims Nexus UI's `/a/<key>` links (`associatedDomains`, and an Android
  intent filter). A tapped link, or `nexus://a/<key>` from the viewer's "Open in the app", opens `src/app/a/[key].tsx`.
  Signed out, it signs in there first. `app.json` names only the production domain, `nexus.inc` (Android).
  `app.config.ts` adds the original Nexus UI host from the build's environment:
  `NEXUS_MOBILE_LEGACY_UI_HOST`, with `NEXUS_MOBILE_LEGACY_GATEWAY_HOST` and `NEXUS_MOBILE_DEV_GATEWAY_HOST` for the
  sign-ins `src/links.ts` moves or keeps. Unset or `""` names none. Nexus's release workflow sets
  Nexus's own, so its iOS and Android builds claim shared links on both hosts.
  Nexus UI serves the matching `.well-known` files once `NEXUS_IOS_TEAM_ID` and `NEXUS_ANDROID_CERT_SHA256` are set
  (Nexus's deploy guide).
  For Android, set `NEXUS_ANDROID_CERT_SHA256` to the app signing certificate from Play Console, rather than only
  the upload certificate. The selected Google-generated key differs from the internal APK's signer: switching
  between those installations requires uninstalling first. Include the internal certificate too if that channel
  must open shared links. Verify the deployed JSON and a tapped production link on the Play-installed build.
- **Screens** (`src/app/`, Expo Router; the list is in `src/app/(app)/(main)/_layout.tsx`): sign-in, a new chat and a
  chat with the answer streaming in, Assistant, Sessions and Archived, Search, Artifacts, Pages, Spend, Coding agents,
  Repositories, Billing, Members, Teams, Models, API keys, Audit, Analytics, Docs and Settings.
- **Code**: your coding sessions, synced from the computers they run on. When phone control is
  enabled on that computer, send a message, stop a run, allow or deny a tool, and answer the agent's questions.
  The phone signs each command and encrypts it to the computer's device key. Only your paired devices can act.
  A queued command says it is waiting until the computer confirms it. A failed or expired send keeps the draft.
  Stop has its own command, so you can stop a run while a message still waits for the computer.
  To deny a tool, you can add a note that tells the agent what to do instead.
  A sleeping computer shows as offline; a Gateway without command support asks for an update. Control and delivery
  are checked only while this screen is open and the app is active. The composer opens when this phone holds the
  session's current key. The unsent message is saved per session, encrypted to this phone's key, and is removed
  when the computer confirms the send. The live stream reconnects when the app returns to the foreground.
  - Scan the desktop's connection QR under **Settings → Devices**. Choose **Approve phone** on the computer.
    This phone keeps its existing key in the Keychain or Android Keystore (`src/device-key.ts`), creating one only when absent. Manual pairing shows a 16-character
    code to type in Settings › Devices on a trusted computer. The phone shows no QR code: only the desktop does. Signing out removes the key.
  - The list shows each session's state, agent, model and cost, which Gateway reads. The title and the repository
    are decrypted on the phone.
  - A session opens on its last 100 events, then follows Gateway's live stream. The timeline is drawn as the
    desktop's is (`src/code-timeline.ts`).
  - Hermes has no WebCrypto, so the kit's end-to-end code runs on the app's own subset, on noble (`src/subtle.ts`).
    `src/subtle.test.ts` runs the kit's e2e, pairing and synced-session tests against it.
- **Agent activity**: Agents shows each run's reported person, usually an email, in both the list and detail.
  Owners and admins can see their organization's runs. Members see their own runs.
  Direct links fetch the authorized run by ID, including runs outside the recent list.
  Lookups stop when their deadline expires or the page closes.
  Temporary failures keep matching cached detail with an error footnote.
  Access denials and missing runs remove that run from the saved cache, including when the page opens again.
  Returning to Agents refreshes its mounted list from the saved cache.
  Responses from an earlier focus cannot restore removed runs.
  A failure without matching cached detail shows an error.
- **Assistant** (S2.5, A21): the person's own agent, Nexus, the one `@nexus` is in Slack. It is the
  drawer's first row, above New session, with the Nexus mark.
  - It asks the agent API with this sign-in's token, through the kit's `createAgentClient` (`kit/src/agent-chat.ts`).
  - The answer streams in, with its sources, model and cost. The conversation's id is kept on the device.
  - It is a page of the drawer beside Chat, not a sheet: Chat's bar, composer, halo, Send and Stop, and haptics.
  - The feed follows an answer only at its end.
  - The drawer's row shows the replies not yet seen, the mark thinking while an answer streams, and Needs you for a
    write that waits or an errand that needs you (A21·c). `src/assistant-row.ts` says what counts.
  - The thread is a session transcript, as the Assistant canvas draws it (`product/nexus/ASSISTANT.md`, section 2).
    The profile header opens it. Nexus writes plain text, your turns are the right-side bubble, and a day marker
    starts each day. There is no avatar and no Delivered or Read. A message that was not sent offers Retry
    (`src/assistant-thread.ts`).
  - The bar names Nexus over "Acme · Assistant". Its one control opens the menu: History, New conversation,
    Errands and Projects in a build with errands, and Assistant settings (`src/assistant-menu.ts`).
  - A send that fails at the network shows "You're offline". What you send then waits, and goes out when the agent
    API answers again. The app has no network-state module, so it tries every 5 s and when it comes to the front.
  - Search's first row, with words typed, is Ask Nexus “…”: it opens the Assistant and sends them.
  - History lists the person's conversations from every surface, Slack included, when the agent API keeps them
    (`NEXUS_AGENTS_SHARED_CONVERSATIONS`). Opening one loads its turns, and the next ask continues it.
  - A 404 on the list hides History. A 503 shows one line, and the chat still works. Each ask sends `surface: "mobile"`.
  - With no conversation kept, the empty chat is the first open (A19), when the agent API serves it
    (`NEXUS_AGENTS_FIRST_OPEN`). It is the web's line and starters, once per person. A starter sends its line.
  - The screen never writes. A write the agent asks for shows read only, and "Review in Nexus UI" opens its approval page.
  - Nexus's release workflow sets `EXPO_PUBLIC_NEXUS_AGENTS_API_URL` from a repository variable, for both
    platforms. Set it in `.env` for local development.
  - Without a URL, the drawer has no Assistant row, and the screen says Nexus is not available here.
  - A 401 from the agent API does not sign you out: it checks the token with its own Gateway.
  - The code: `src/app/(app)/(drawer)/(chat)/agent.tsx`, `src/assistant-bubbles.tsx`, `src/agent-chat.ts`, `src/conversations-sheet.tsx`,
    `src/agent-conversations.ts` and `src/agent-first-open.tsx`.

The store accounts are not in `app.json` (open-source export, B12): `app.config.ts` reads `NEXUS_MOBILE_EAS_PROJECT_ID`,
`NEXUS_MOBILE_EAS_OWNER`, `NEXUS_MOBILE_IOS_BUNDLE_ID`, `NEXUS_MOBILE_APPLE_TEAM_ID` and `NEXUS_MOBILE_ANDROID_PACKAGE`
from the environment, and falls back to the `production` profile's `env` in an `eas.json` beside it, when there is one. Unset, a setting is
left out, so a fork builds under its own accounts. Nexus's release workflow sets Nexus's from repository variables.

The icon and splash images in `assets/` are neutral stand-ins, a white ring on grey (`python3 -I assets/make_stand_ins.py`
draws them). Nexus's own icon and splash come from the Nexus mark, which is not in this repository (TRADEMARKS.md): a
store release copies them from the private `nexus-xyz/nexus-brand` repository and stops when any image is still a
stand-in (`.github/workflows/mobile-release.yml`). A rehearsal, and a fork, builds with the stand-ins. Without an image
file, `app.config.ts` leaves its setting out, and a build shows Expo's default.

Color is brand v4's, with Nexus UI's light and dark values.
`src/theme.ts` names the source of each value. Text is in the metals and clears 4.5:1 in both themes. The primary button
is Black on light and White on dark. The phone shows no route hue, because it shows no routed work yet.
The one accent is Nexus UI's ice blue, on a new chat's composer (`src/composer-halo.ts`).
While an answer comes in, a soft glow breathes around the composer, as in Nexus UI's chat.
Import `Switch` and `TextInput` from `@/controls`, not from React Native, which draws them iOS green and blue.

Not yet: one token file shared with Nexus UI (`src/theme.ts`
copies its values, so a change there is made here too), and the App Store and Play Store.

## Run it

```sh
npm install
npx expo run:ios       # needs Xcode; or run:android with Android Studio
```

`ios/` and `android/` are generated and ignored. `modules/crash-reports/` is the app's own native module (MetricKit on
iOS, `ApplicationExitInfo` on Android): `expo run:ios` and `expo run:android` link it, and Expo Go does not, so Expo Go
reports JavaScript errors only.
Use a development build for sign-in: Expo Go does not register the app's callback scheme.

### Test desktop controls in Expo Go

The sidebar's mixed **Sessions** history opens Chat and paired computer sessions. The computer must run a build with phone
control and encrypted sync, which is on by default. Settings › Devices › Remote control must be on.
The Gateway must include the command mailbox routes. Existing local sessions made before sync was enabled do not
upload automatically.

For an isolated local Gateway, set its Wi-Fi address in `.env.local`, then restart Metro:

```sh
EXPO_PUBLIC_NEXUS_DEV_GATEWAY=http://<computer-wifi-address>:4640 npx expo start --go --lan
```

Start and approve a device sign-in on that Gateway, then open
`exp://<computer-wifi-address>:8081/--/dev/connect?code=<device_code>` on the phone. The development route exchanges
the single-use grant through the kit and opens the new-session composer. API keys are never included in the Metro bundle. This mode
keeps separate sign-in and device keys in SecureStore, plus a separate SQLite store for caches and drafts;
remove the environment setting and restart Metro
to return to the normal sign-in. The connection route is unavailable in release builds.

Choose **Settings → Connections → Computers → Connect a computer** and scan the development desktop's **Connect a device** invitation.
Manual device-code pairing remains available as a fallback. A new session with
sync enabled appears in the list. A development copy of an older session must be explicitly bound to the local
Gateway's verified person; continuing it as a native fork preserves the original transcript.

`npm install` applies `patches/` (patch-package). The one patch gives the sidebar's drawer
(react-native-drawer-layout) a faster spring; a comment in it says why. When you upgrade a patched package, redo the
patch or delete it.

## Releases

`.github/workflows/mobile-release.yml` builds Nexus's release builds from a `mobile-v*` tag, in the `mobile-release`
environment, with Nexus's signing keys and store accounts. A run by hand with `rehearsal` builds an unsigned Android
release and publishes nothing. Pull requests build an unsigned debug APK (`.github/workflows/mobile.yml`).

## Checks

Custom haptics confirm navigation, model and segmented selections, chat controls, long presses and visible
results such as copying or archiving. Existing chat success and error feedback shares the same helper.
On iOS, turn them off in Settings → General → Appearance → Haptic feedback.
On Android, open Appearance from Settings → Preferences. This device preference survives sign-out.
Feedback stays quiet while the app is inactive, and events within 80 ms are coalesced, with task outcomes taking
priority over taps. Android uses native haptic effects; iOS uses its standard feedback generators. System
controls keep their own device-controlled feedback. Typing, streaming tokens and scrolling add no haptics.

- `npm run typecheck` here.
- `npx expo export --platform ios --platform android` bundles both apps, which shows Metro resolves `@nexus/kit`.
- `src/*.test.ts` runs in the repository's vitest, with the kit's tests: `npm ci && npm test` at the repository root.
- `npm run prebuilt-symbols` checks the Swift symbols that each precompiled Expo framework imports from another
  precompiled Expo framework: each must be defined there. A miss ends the app at launch, before any JavaScript runs.
  It does not check C or Objective-C imports. It needs macOS's `nm` and `tar`. It fails when it finds no
  framework, so a change in Expo's layout cannot make it pass. EAS runs it on every iOS build
  (`eas-build-post-install`). When an import is missing, run `npx expo install --fix`.

### Android release sign-in

Test an installed release APK, not Expo Go. Keep the account owner's credentials on the device.

1. Start signed out. The app must show Google and email sign-in. Signing out must return to that screen,
   not the computer scanner. The index route handles launches, and authentication guards redirect directly to
   sign-in or chat when the session changes. Pending computer invitations keep their destination.
2. Continue with Google from Nexus. Complete Google's authentication and any account verification in the browser.
   Signing in to Chrome alone does not complete Nexus sign-in.
3. Confirm the account in Nexus UI if asked. The browser must return to the app, which must load the account's data.
   If Android offers several installed apps for the callback, select the intended Nexus installation.
4. Restart the app and verify the account stays signed in. Log out, start sign-in again, and verify Nexus UI offers
   Continue and Use another account. Finish sign-in and leave the app signed in for screenshot work.

If Google repeatedly opens Chrome's account settings, check the browser's account verification. During Android
release testing, Chrome returned Google's HTTP 204 `X-Chrome-Manage-Accounts: action=ADDSESSION` handoff because its
stored Google sign-in was stale. Signing Chrome out locally and letting the account owner complete Google's
verification resolved it in the unchanged release APK. Chrome may say "Your stored sign-in info might be out of date."
Treat this as a browser-account problem. Keep the system-browser PKCE flow and the app's state validation intact.

## App Store builds

What the App Store build declares, and what it shows in each storefront:

- The app uses OS encryption and standard algorithms from `@noble/*` for synced Code sessions. It omits
  `usesNonExemptEncryption`, so App Store Connect asks for the export determination instead of assuming exemption.
  Complete the [encryption questionnaire guide](ENCRYPTION.md) for the selected build.
- `app.json` drops `expo-secure-store`'s default Face ID usage string (`faceIDPermission: false`). The app never asks
  for Face ID, so the prompt would have no reason. A read with `requireAuthentication` needs the string back, with its
  reason.
- `package.json` builds `expo-image-manipulator` from source on iOS so CocoaPods includes SDWebImage's privacy manifest.
- `app.json` sets `supportsTablet: false`. The App Store build is for iPhone only, so it needs no iPad screenshots.
  An iPad runs it at iPhone size.
- The first App Store launch excludes all EU storefronts and China mainland. Outside the US, Apple forbids any text that points to a purchase outside the
  app. So no notice says where to choose a plan or buy credits (`src/plan.ts`), on Android too.
- Billing follows the store (`src/storefront.ts`). In every storefront, an owner turns auto top-up off and
  lowers the usage limit in the app. Neither buys anything. With no way to pay, Billing meters the free start and
  lists what stays free: the organization's own provider keys, and people's own Claude or ChatGPT plans.
- On the US App Store storefront only, Billing has rows that open Nexus UI's Billing in Safari. Guideline 3.1.1(a)
  allows this there with no entitlement, if the link opens outside the app.
- The storefront comes from StoreKit (`modules/store-country`), not the device region. Expo Go, Android and an
  unknown storefront show no link.
- Android has no link: a US link-out on Google Play needs Play's external content links program, its APIs and its fees.

## App Review notes

App Store Connect keeps the notes for App Review. Nothing here sends them: paste this text into the version's "Notes"
field when you submit a build. This text is the only copy: change it here first, then in App Store Connect
.

- Put the demo account's email and password in App Review Information › Sign-in information, never in this file.
- Before you submit, sign in to the selected build with the demo account on a physical iPhone and on an iPad, each
  new to the account. App Review rejected 0.1.0 build 10 on an iPad Air with iPadOS 27.2.1: the sign-in kept loading
  (guideline 2.1(a)).
- Paste the whole quoted block below into Notes, sign-in steps first. Build 10's Notes held only the content
  paragraph, with no sign-in steps.
- Submit only after the Nexus UI release that carries the sheet's time limits is live (`stall.ts`). The
  sheet loads from production, so an old Nexus UI keeps the old sheet, whatever the build.
- Submit only a build whose processing has finished, and check its number against the mobile release record.
- Guideline 1.2 names apps that generate content with AI.

> Nexus is an AI gateway for companies. An organization's people use AI models from several providers under the
> organization's own model policies and budgets. The iPhone app is a client for a Nexus account.
>
> Sign-in: tap "Sign in with email". Enter the demo account's email and password from the Sign-in information.
> Tap Continue. If the sheet asks you to confirm the account, tap Continue again.
> The sheet closes and the app opens the demo organization.
>
> Nexus is by invitation only. "Continue with Apple" and "Continue with Google" ask a new account for an invitation
> code, so please use the demo account. If a sign-in step takes longer than 30 seconds, the sheet or the app shows a
> message. Please send us that message, and we will answer the same day.
>
> AI providers: before the first message, the app says which provider receives it and asks for permission.
> Settings › Privacy › AI providers withdraws that permission.
>
> Nexus answers with AI models from several providers. Each provider's own safety system filters the questions and
> answers. An organization's owners and admins choose which models its members may use.
>
> To report an answer: under any answer in a chat, tap the More button (…), then Report, then a reason. The report
> goes to the Nexus team with the answer, the model that wrote it, and the sender's name, email and organization.
> Settings › Send feedback reaches the same team.
>
> Contact: Settings › About › Support opens <https://nexus.inc/support>.
>
> People in Nexus belong to an organization. In Nexus on the web, its owners and admins remove a member, and the
> member then loses access to the organization's chats and shared content.
>
> Account deletion: Settings › Account › Delete account on iOS. Please test it with a new account, not the
> demo account, so the demo account stays usable.
>
> Billing (Settings › Billing, owners only): the app sells nothing. On the United States storefront, Billing has
> links that open the organization's billing page on the Nexus website in Safari, under guideline 3.1.1(a). On every
> other storefront the app shows billing status only, with two controls that charge nothing: turning auto top-up off,
> and lowering the monthly usage limit.

The report reaches the channel that `NEXUS_FEEDBACK_SLACK_WEBHOOK_URL` names on the Gateway. Without it, a report
fails with "Reports are not set up on this server yet."

## App Privacy

App Store Connect's App Privacy answers are kept in `app.json`, under `ios.privacyManifests`. Prebuild
writes them into the app's `PrivacyInfo.xcprivacy`. App Store Connect does not read that file, so a person with the
App Manager role enters the same answers in App Store Connect, under App Privacy.

The manifest also declares `AC6B.1` for the managed configuration module's use of `UserDefaults`.

The app does no tracking. Each data type below is linked to the person, because each request goes with the sign-in.

| Data type | What the app sends | Purpose |
| --- | --- | --- |
| Name, email address | Sign-in, through the identity provider (Google, Apple, or an email and a password). Feedback and a reported answer name the person. | App functionality |
| User ID | Each Gateway call. | App functionality |
| Photos or videos | The photos attached to a question. A photo loses its location before it is sent. | App functionality |
| Other user content | Chats, pages and the conversation with Nexus in the Assistant. | App functionality |
| Customer support | Send feedback and Report an answer (`POST /v1/diagnostics/feedback`), to Nexus's channel in Slack. A report carries the answer and its model. | App functionality |
| Other usage data | The Gateway's usage record of each model call, and its growth events. | App functionality, analytics |
| Crash data | Native crashes (MetricKit on iOS, `ApplicationExitInfo` on Android) and fatal JavaScript errors (`src/reporting.tsx`), unless the person turns reports off in Settings. | App functionality |
| Other diagnostic data | JavaScript errors that are not fatal. | App functionality |

App Store Connect does not count these as collected:

- Audio. Dictation streams the voice to the speech-to-text provider, which keeps no audio or text (the privacy
  policy, "Dictation"). Apple counts data as collected only when it is kept longer than the request needs. Voice
  mode sends Nexus only the words. The platform's recognizer hears the audio only when the phone cannot do it alone.
- Location, device IDs, contacts and purchases. The app sells nothing.
- The paired phone's device key. Pairing sends the Gateway the key's public halves and a fixed name ("iPhone"), a credential
  for synced sessions that the app makes at random, not an identifier of the device.

When a change sends a new kind of data from the phone, change `app.json`, the privacy policy
(`ui/src/content/legal/privacy.md`) and the answers in App Store Connect and Google Play together.
`src/app-privacy.test.ts` checks that `app.json` uses Apple's names and declares no tracking.

### Google Play Data safety

Google Play's Data safety form is entered separately in Play Console for `xyz.nexus.android`, under
**App content → Data safety**. Saving the completed form stages it in Publishing overview; Google reviews it with
the app before it appears on the listing. A saved form is not evidence that a release passed review.

The saved declaration uses these Google data types and purposes:

| Data type | Collection | Purpose |
| --- | --- | --- |
| Name | Required | App functionality, account management |
| Email address | Required | App functionality, account management, developer communications, fraud prevention/security/compliance |
| User IDs | Required | App functionality, account management, analytics, fraud prevention/security/compliance |
| Other in-app messages | Optional | App functionality |
| Photos | Optional | App functionality |
| Files and docs | Optional | App functionality; retrying a message can resend files attached on the web |
| App interactions | Required | App functionality, analytics, fraud prevention/security/compliance |
| In-app search history | Optional | App functionality; Chat search sends its query to the Gateway |
| Other user-generated content | Optional | App functionality; pages, feedback and reported answers |
| Crash logs, diagnostics | Optional | Analytics; Google includes diagnosing crashes in this purpose |
| Voice or sound recordings | Optional, ephemeral when dictation is enabled | App functionality |

[Google's definitions](https://support.google.com/googleplay/android-developer/answer/10787469?hl=en) differ from
Apple's: sending data off the device counts as collection even if the recipient keeps it only for the request.
Declare dictation audio as ephemeral collection when enabled; do not copy Apple's audio exclusion into Play.
`src/dictation.tsx` streams audio to the speech provider through `kit/src/dictation.ts`. The public privacy policy
currently marks dictation as not yet enabled; check its status and the Gateway configuration before release.

The saved form declares no sharing under Google's service-provider and user-initiated/consented-transfer
exceptions. AI providers still receive messages and attached photos: `src/ai-consent.ts` names them and asks before
the first send. The privacy policy describes contracted providers, including speech, identity, hosting and team
messaging. Recheck these exceptions if a provider uses data for its own purposes or the consent flow changes.

Security answers declare encryption in transit, password and OAuth account creation, and deletion requests at
<https://nexus.inc/legal/privacy#11-your-rights-and-choices>. The privacy policy is
<https://nexus.inc/legal/privacy>. Before sending a release for review, compare the form with the signed build,
its SDKs, enabled server features and the live policy. Change the form when those change.

## Computer sessions

The sidebar, Sessions history and new-session suggestions mix Chat with encrypted computer sessions. A session row,
header and composer retain its origin. Search queries Chat messages through Gateway and matches computer titles
already opened on this phone. It does not send decrypted computer titles to server search or the ordinary Chat cache.

On the computer, open **Settings → Devices** and choose **Show connection code**.
On the phone, sign in and open **Settings → Connections → Computers → Connect a computer**.
If a system link opens the signed-out app, the sign-in screen keeps the invitation in memory until sign-in completes.
Scan the QR code, then choose **Approve phone** on the computer. The preview shows **Approve device**.
Once the phone verifies the computer is awake with remote control enabled, choose **Open computer**.
If this is a new computer on an account with trusted devices, scan with a phone the account already trusts.
The first device on an account becomes trusted during setup.

To trust the computer for encrypted sync, open **Settings → Devices → This device** on the computer.
Choose **Trust this computer** for a first device. Otherwise, choose **Request approval**.
On a trusted phone, open **Computers**: each computer that waits for approval is listed under **Waiting for approval**.
Tap it, check that its code matches the computer's screen, and tap **Approve computer**.
Or, on the computer, choose **Show connection code**. On a trusted phone, open **Connect a computer** and scan it.
A trusted phone that scans it approves the computer too. This device shows no QR code of its own.
You can also enter the code under **Settings → Devices → Add a device** on a trusted computer.
On the computer you are setting up, match the approving device’s fingerprint and click **Confirm**.
Turn on **Remote control** to allow incoming commands from your trusted devices. Turn it off to pause them.

The phone can also open the scanner from **Run on**.
Opening the newly connected computer from there brings the unsent text draft.
The phone controls the session. Files and tools stay on the computer, which runs the agent.

Use **Copy connection link** if the phone cannot scan the QR code.
For desktop-to-desktop linking, start both desktop apps with `NEXUS_REMOTE_PAIRING_PREVIEW=1` first.
On a second desktop, paste that link in **Settings → Devices → Other computers**.
Choose **Link computers** on the second desktop. Approve the request on the first to connect in both directions.
Installed apps accept `xyz.nexus.app://connect#invite=…`. Expo Go links use their Metro URL.
The invitation is accepted only for the same Gateway, person and organization.
Nexus checks both device keys against the signed invitation and request.
Phones enrolled outside the invitation can still confirm their earlier approver.

For a new computer session, Run on lists verified named computers with Awake or Offline state. Choose a saved
project and review the computer's agent, model, permissions, workspace and billing route. The phone starts through
signed encrypted requests and opens the returned synced session ID. It does not execute coding sessions on the
phone or offer an unsupported Cloud start target. Existing computer sessions use the same reply, stop and approval
controls on their original computer.

On computers that advertise model settings support, **Model** and **Who pays** open native sheets populated by that
computer. A new session can choose an available subscription, API key or Nexus account route for its project's
configured agent. Choices and drafts stay encrypted on the phone; stale or unavailable choices require a deliberate
selection before starting. Existing sessions keep their original payment route and allow supported model and effort
changes. The composer shows the effective model while a saved change awaits the agent's acknowledgement. Older
runners ask for an update and continue to use their existing start and control capabilities.

Native drafts and exact pending start and settings envelopes are self-encrypted to this phone's key before SQLite storage.
Requests are retained before sending; reopening resumes receipt checks without a new POST. A queued or claimed
request remains pending until an authoritative result. An uncertain start must be checked in history before the
person explicitly stops tracking or starts again. Sign-out removes the local journal with the person's other data.
A confirmed creation keeps its session ID and draft in that encrypted journal while its first history upload is
pending. The composer checks for published encrypted history before opening it, including after a phone restart.
After 30 seconds without history, the composer says why: sync may be off on that computer, or the organization keeps
session content on the computer. It then offers **Open session** and **Start another**. A computer with remote control
off says so, apart from an offline one. A saved draft that does not open offers **Discard saved draft**.
