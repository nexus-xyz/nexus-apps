# Design canvases and slide decks

Both are pages: an HTML or React page with marked sections. Read `pages` first for how to publish one.
Read this page before you write screens, mockups or slides.

## A design canvas

Use it for screens, mockups, or a UI to iterate on.

- Mark each screen as an artboard: `<section data-artboard="Home" data-width="1440" data-height="900">`. Use 390 by 844 for a phone.
- The page then opens as a canvas that the reader zooms and pans. Screens that share a parent are in one row.
- On a phone, drag to pan and pinch to zoom. Choose Interact to scroll or use controls inside a mockup.
- Give a board `data-id="L1"` and `data-status`: `final`, `proposed` or `superseded`.
- Name the parts of a board with `data-layer`.
- Group boards into canvas pages with `<section data-page="Mobile">`.

## Working beside people

People may edit the canvas while an agent writes. Their unsaved edits are kept: their canvas merges them onto the agent's version and saves the result as the next version.
An agent's write shows the agent on the canvas, next to the person it works for, with a marker on each board it changed. The marker goes after two minutes without a write. It never stops a person editing.
Over the API or MCP, an agent names itself with `agent` in the write's body, such as `"agent": "Claude"`. Read the latest version before the next edit, because people may have changed it.

## Variants

To offer alternatives to a board, add boards in a new row under its row.
Give each one `data-status="proposed"`, `data-variant-of="L1"` (the original's `data-id`) and `data-note`, one line on what it tries.
The reader keeps one, and it replaces the original.

## Tweaks

Declare a few values that the reader tunes live. Each is a CSS variable on `:root` with a comment. Use the variables in the boards.

```css
:root {
  --gap: 24px;       /* @tweak range 8 64 4 "Gap" */
  --accent: #2F6FEB; /* @tweak color "Accent" */
  --density: cozy;   /* @tweak choice compact|cozy|roomy */
}
```

## The design system

Before you write a design, read the organization's design system.

```sh
nexus design-system show           # the default: its link tag, its tokens and its notes
nexus design-system show <name>    # another one
nexus design-system list           # all of them, and which one new designs use
```

- Put the link tag in the design. Use the tokens as `var(--name)` for colours, font sizes, spacing and radii.
- The built-in `@nexus/brand` is the default until the organization picks another.
- `nexus pages check <page-id>` checks a published design against its design system. It finds colours, font sizes and spacing that are off its tokens, and text below WCAG AA contrast. It changes nothing.
- Owners and admins make a design system from a repository with `nexus design-system sync <path>`. It reads the tokens, the `DESIGN.md` and the component names.
- Owners and admins choose the default with `nexus design-system use <name>`.
- GPUI shows Design systems under Settings › General. Read tokens, themes, notes and components there.
- Only owners and admins can change its default. Sync new systems from their repository with the CLI.

## A slide deck

Use it for a talk, a pitch or a walkthrough.

- Make each slide a `<section data-slide="Title">`, laid out for 1920 by 1080. `data-width` and `data-height` change the size.
- Put the speaker notes in an `<aside data-notes>` inside the slide.
- The page then opens as a deck. The reader steps through it with the arrow keys and presents it full screen.
- Write no navigation script of your own.

## Export

```sh
nexus pages export <page-id> --format pdf --out deck.pdf
nexus pages export <page-id> --format png --board L1
```

| Format | What it writes |
| --- | --- |
| `png` | One board. A page with several boards needs `--board` |
| `svg` | One board as shapes and text you can edit in Figma. A page with several boards needs `--board` |
| `pdf` | One page for each board |
| `pptx` | One slide for each board |
| `zip` | Every board |
| `html` | The page as one file that opens offline |

`--board` takes a board's `data-id`, its name, or its number from 1. For a deck, it takes a slide's title or number.

## Design process

- Before you draw, say your plan in one line: the feeling, the palette with a job for each color, the type, the layout.
- Build one visual group at a time.
- After each write, run `nexus pages check <page-id>`, then look at a rendered board (`nexus pages export <page-id> --format png --board <board>`, or `nexus pages frame <page-id>`) before you continue.
- Fix overflow, clipped text and misaligned rows as you find them.
- Use the design system's tokens, never raw colors, sizes or spacing.
- Say you looked only at what you rendered after your last change.
- Finish with a refinement pass against this looks-generated list, and fix every hit:
  - a headline with one recolored word
  - formula copy such as "X, but for Y"
  - an eyebrow label on every section
  - boxes inside boxes
  - a floating toast or badge for decoration
  - fake proof: invented logos, numbers or quotes
  - flat surfaces with no light, depth or material
  - navy with electric purple, or gradients with heavy shadows
  - details that differ between like elements (radii, icon sizes, gaps)
