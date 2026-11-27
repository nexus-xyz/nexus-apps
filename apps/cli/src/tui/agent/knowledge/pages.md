# Pages

A page is a standalone file that a person publishes to their Nexus organization.
It is HTML, Markdown, or one React component. It is private until its owner shares it.
Use a page for output that is easier to view than to read in the conversation: a report, a dashboard, a chart, a diagram, a design or a slide deck.

## Publish and change

Write the file in the workspace first. Then publish it.

```sh
nexus pages publish report.html             # a new page, version 1
nexus pages update <page-id> report.html    # the next version of the same page
nexus pages list
nexus pages get <page-id> --source --out report.html
nexus pages versions <page-id>
nexus pages restore <page-id> 1
```

- `publish` prints the page's id (`pag_…`) and a line `View it: <link>`. Give the person that link.
- The format comes from the file name. `.md` and `.markdown` are Markdown. `.jsx` and `.tsx` are React. Any other name is HTML. `--format` names it.
- The title is `--title`, else the file's `<title>` or first `#` heading, else the file name.
- For a change that the person asks for, update the same page. Do not publish a second page.
- `--base N` on `update` refuses the write when someone wrote a newer version than N.
- Versions never change. A version holds at most 16 MiB, and a page keeps at most 500 versions.
- Do not paste a long page into the conversation when a page is clearer.
- In a Code session of Nexus for desktop, read `desktop` before you publish.

## See how it renders

- `nexus pages frame <page-id>` prints a signed address that renders the page with no sign-in, for five minutes. Open it with a browser tool when you have one.
- `nexus pages open <page-id>` opens the page in the person's browser.

## Find pages in the Library

The Library in Nexus UI and Nexus for desktop has All, My drafts, Shared with me and Shared by me.
My drafts contains only pages you own.
Pages shared with you stay under All and Shared with me.
Filter by Sites, Docs, Sheets, Boards or Chats; the tab counts follow that kind.
Each page appears once, even when it has several shares.
A row's **…** menu acts without opening it: Open, Copy link, Share…, Version history…, Download, Revoke link…, Delete….
On the desktop, it also lists the other shares.
Search narrows the selected view by title, sharer or audience.
Opening your draft shows your page; opening a share follows that share's version and access.
A share from another organization opens its reader in the browser.

## Reference an artifact in a session

In the desktop Library, choose **Add to a session…**, then a new or recent session.
Or choose **+** → **Add artifacts…** in a composer.
Each artifact waits as a versioned chip until you send.
The message lists its title, kind, id, version and link.
The desktop reads sources with its sign-in, up to 4 MB each.
A chat gets data files. A Code agent gets files under "Attached files".
For missing files, the message names the desktop Gateway for `nexus pages get`.
Click a sent chip to open it in the Workbench.
Another organization's share cannot be added.

## Share

Share a page only when the person asks.

```sh
nexus pages share <page-id> --org                      # everyone in the organization
nexus pages share <page-id> --team web                 # or --people a@x,b@y
nexus pages share <page-id> --to ada@partner.example   # named people outside, by email
nexus pages share <page-id> --public --expires 30d     # anyone with the link
nexus pages share <page-id> --team web --pin 2         # readers see version 2, not the latest
```

- `--team` shares with that team and with the teams under it.
- People outside open the link signed in with the email that you named. Nexus does not email them: the person sends the link.
- The organization's settings can forbid a kind of share. The command then says why.
- The CLI cannot give edit access, comment or remix. Nexus UI does those.
- `nexus pages delete <page-id> --yes` deletes the page, its versions and its shares for good. Run it only when the person asks for exactly that.

## What a page may contain

| Format | Rules |
| --- | --- |
| HTML | A full document. Scripts load only from cdnjs.cloudflare.com, cdn.jsdelivr.net/npm/, unpkg.com, cdn.tailwindcss.com and code.jquery.com. Fonts load from Google Fonts |
| React | One file with one default-exported component, `export default function App() { … }`, styled with Tailwind classes |
| Markdown | Prose. It runs no scripts |

- A page can fetch only from its own host. It cannot submit forms, open windows, navigate away or download files.
- A page cannot load images from other sites. Put images in the file as data URLs or inline SVG.
- A React page may import only `react`, `react-dom`, `recharts`, `lucide-react`, `d3`, `three`, `lodash`, `papaparse`, `mathjs` and `chart.js`.
- Gateway refuses a page with a password field, a form that submits somewhere, a meta refresh, an iframe, an object or embed tag, or `eval` of decoded text. The error says which.

## What a page can call

An HTML or React page has `window.nexus`, a bridge to the reader's own session.

- `await window.nexus.complete(prompt)` asks a model and returns the answer's text. It also takes `{ messages: [{ role, content }], maxTokens }`.
- The reader pays for each call. So call it on a user action, and show the error when it throws.
- `window.nexus.storage` keeps text by key: `get(key)`, `set(key, value)`, `delete(key)` and `list(prefix)`.
- A stored value is the reader's own. Pass `{ shared: true }` last for one copy that every reader sees.
- A page opened from a public link has no bridge. A Markdown page has none either.

## Designs and decks

An HTML or React page whose screens are marked as artboards opens as a design canvas.
One whose sections are marked as slides opens as a slide deck. See `design`.

## When pages are off

An organization can turn pages off with its `artifacts.pages` setting. Then `publish` is refused.
Say so, and leave the file in the workspace.
