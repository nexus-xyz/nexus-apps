# The person's assistant

Each person has an assistant of their own, with its memory and its saved model.
The apps call its place "Assistant". The one the person talks to is Nexus. In Slack it is `@nexus`.
It is not you. You are the coding agent of this session. See the three agents in `overview`.
Read this page when the person asks about `@nexus`. Slack, routines and the inbox are in `slack-and-inbox`.

## The assistant

```sh
nexus agent ask "What did I decide about the launch?"
nexus agent ask --conversation <id> "And the date?"
nexus agent chat                                       # one conversation, in a terminal
```

- It answers as the person, through Gateway, under their model policy and budget.
- A write that asks first runs only after the person approves it. It writes as an agent link of the person, never as their own key.
- In the Assistant pane, the person approves or declines the write on its card. Elsewhere it prints the Nexus UI page for it.
- A mail it sends as the person through Outlook ends with "Sent using Nexus", as its card shows.
- The person turns that line off under Assistant › Approvals: "Sign messages with Sent using Nexus".
- A message it sends as itself ends with "Sent by Nexus, <name>'s assistant" instead.
- It needs the agent API. The CLI reads its address from `NEXUS_AGENTS_API_URL`.
- Nexus UI and Nexus for desktop show it as the Assistant pane, where the agent API is set up.
- In Nexus UI the pane reads like a text thread. Its ⋯ menu starts a new conversation and opens the other views.
- In Nexus UI a hover or a long-press on a message adds a reaction, which the browser keeps. The ⋯ menu mutes sounds.
- The Assistant thread shows the step that runs, up to three quick replies under a question, and each errand's card.
- A question can need an account the person has not connected. The answer then names the connector and its link.
- In the Assistant pane, that answer ends with a Connect card, which opens the connector in Nexus UI.
- ⌘I, or a click on the pane's Nexus header, opens its contact card. Its rows link to the Assistant's views.
- In Nexus UI's Assistant pane, the composer's voice button starts voice mode: a hands-free call with Nexus.
- Each spoken turn is a normal ask in the same conversation. Nexus says the answer, and both sides show as captions.
- End or Esc leaves voice mode. A summary card with the transcript lands in the thread. A write still waits on its card.
- Voice mode needs Gateway's speech-to-text service. Without it, voice mode is off.
- Dictation is free. Nexus's own voice for the answers costs about 4.5 cents a minute, from the free start or the balance.
- When the balance or a budget does not cover it, Nexus answers with the device's voice. Nothing else stops.
- An errand is a job the person handed it. The `errands` topic covers errands, the Errands view and projects.
- Do not answer for it, and do not claim its memory. Ask it with the command when the person wants its answer.

## On the phone

- Nexus for iPhone and Android has the Assistant as the drawer's first row: the brief, quick replies and errand cards.
- The ⋯ button in its bar opens the menu: History, New conversation, Errands, Projects and Assistant settings.
- Account › Assistant opens Memory, Discover, Proactivity, Privacy and Passwords.
- Memory lists the memory files. Tap a fact to see its source, edit it or delete it.
- On the phone, the person can ask "Text Ana that I'm running late" or "Invite Ana to Nexus".
- The Assistant shows a text card. The phone finds Ana in its contacts, which never leave the phone.
- The card shows To and the exact text, with Edit, Cancel and Send. An invitation ends with the person's referral link.
- Send opens the phone's Messages app. Nothing goes until the person taps Send there.
- Account › Referrals has Text a friend: Messages opens with the referral link, and the person picks who gets it.

## How often it writes first

- The person sets it in Nexus UI: Assistant, then Approvals, then Proactivity.
- One dial: Quiet, Balanced or Hands-on. Balanced is the default.
- Quiet sends only what the person asked for, such as a watch, and approvals. Balanced also sends the morning brief.
- Quiet hours hold every notice until they end. More than one push in 15 minutes becomes one.
- A brief in the Assistant pane has "Why am I seeing this?", with "Fewer like this" and "Turn off".

## Passwords and payments

- The person opens it in Nexus UI: Assistant, then Passwords and payments.
- Nexus never sees, stores or repeats a password or a card number. The password manager fills a login in.
- Each login has a grant: each use, or until a date. Each use asks the person first.
- The page also shows the card, its limit per merchant, and an activity log of each use.
- No password manager or card is linked yet, so the page says that nothing is linked.

Slack, routines and the inbox are in the `slack-and-inbox` topic.
