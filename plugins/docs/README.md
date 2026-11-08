# vyasa-docs

Documentation sites for [Vyasa](https://github.com/vyasa-cms/vyasa). Point it
at a folder of Markdown in a GitHub repository and it keeps a **Docs**
section of your site in step with it: a sidebar that follows the folder
structure, a table of contents, previous and next links, breadcrumbs,
versions, search scoped to the docs, and a few documentation blocks (tabs,
steps, code groups, a parameters table, a file tree, callouts).

The pages are ordinary Vyasa entries of the `doc` content type, served at
`/docs/<path>`. They are read-only in the editor ("Synced from vyasa-docs.
Edit it at the source.", with an **Edit source** link) until someone presses
**Detach**; after that the importer never touches that page again.

## Install

From the marketplace (Admin, Plugins, Browse), or upload the `.vyplugin`
(see Building below). Enable it, then fill in the settings. Vyasa 0.3 or
newer is required (`min_host_api = 2`).

The plugin asks for these capabilities, and nothing else: `log:write`,
`kv:read`/`kv:write` (its own state), `entries:sync` and `db:publish:posts`
(to write and publish the pages), `net:fetch:api.github.com` and
`net:fetch:raw.githubusercontent.com` (to read the repository),
`tasks:extended` (a sync can run for up to a minute) and `assets:script`
(the small script for tabs, copy buttons and the contents highlight; pages
read correctly without it).

## Settings

| Setting | Meaning |
|---|---|
| Repository | `https://github.com/owner/name` |
| Branch | Default `main` |
| Folder | Default `docs`; leave empty to use the repository root |
| Token | A fine-grained token with Contents: read, for a private repository or a higher rate limit. Write-only: it is never shown again |
| Webhook secret | The key shared with the GitHub webhook (below). Write-only |
| Check every (minutes) | 5 to 1440, default 60. The webhook makes changes appear at once; polling is the safety net |
| Latest version | The label of the version served at `/docs`; empty for unversioned docs |
| Latest label | How the version switcher names it, default `Latest` |
| Older versions | One `label = folder` per line, for example `v1 = docs-v1` |
| Docs title | The breadcrumb root, default `Docs` |
| Search placeholder | Default `Search the docs` |

## Webhook

In the GitHub repository open Settings, Webhooks, Add webhook:

- Payload URL: `https://<your-site>/api/v1/plugin/vyasa-docs/webhook`
- Content type: `application/json`
- Secret: the same value as the plugin's **Webhook secret**
- Events: "Just the push event"

A delivery without a valid signature is answered `401` and does nothing. A
push to the configured branch queues a sync that runs within the minute;
other branches and other events are acknowledged without work.

## The Markdown dialect

Pages are CommonMark with tables, strikethrough, task lists and GitHub
alerts (`> [!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]`, `[!CAUTION]`).
The first `# Heading` is the page title.

**Front matter** is a small subset of YAML between `---` fences, one
`key: value` per line (no nesting, no multi-line values): `title`,
`description` (the excerpt), `position` (also `order`, `sidebar_position`,
`weight`), `version`, `hide` (also `hidden`, `hide_from_nav`), `slug`,
`section` (also `group`: a sidebar heading above the page), `updated`
(also `last_updated`, `lastmod`). Unknown keys are ignored; a line that
cannot be read becomes a warning in the sync report.

**Order.** A folder is a node and its `index.md` (or `README.md`) is its
page. In a folder, names listed in `_meta.json` come first:

```json
{"pages": ["install", "configure"], "title": "Guides"}
```

or, as an ordered object with titles:

```json
{"install": "Install", "configure": {"title": "Configure", "hidden": true}}
```

then pages with a `position` (or a numeric file prefix such as `02-`), then
the rest by name.

**Components** are recognised at the start of a line, outside code fences:

```md
<Callout type="warning" title="Careful">This drops the table.</Callout>

<Tabs items={["npm", "pnpm"]}>
<Tab>

    npm install

</Tab>
<Tab>

    pnpm add

</Tab>
</Tabs>

<Steps>
<Step>

Install it.

</Step>
<Step>

Run it.

</Step>
</Steps>

<Params>
<Param name="timeout" type="number" default="30">Seconds to wait.</Param>
<Param name="url" type="string" required>Where to connect.</Param>
</Params>
```

A fence with `tree` as its info string becomes a file tree (two spaces per
level, a trailing `/` marks a folder, ` # note` adds a comment); consecutive
fences with the same `group="name"` become one code group with a tab each
(`title="..."` names the tab, else the language is used).

## Versions

Two ways to keep old documentation online:

1. **A folder per version.** Set *Latest version* to `v3`, keep the current
   docs in `docs/`, and list `v2 = docs-v2` and `v1 = docs-v1` under *Older
   versions*. `/docs/guides/install` is the latest page and
   `/docs/v2/guides/install` the old one; an old page shows a banner linking
   to its counterpart and names that counterpart as its canonical address.
2. **A `version:` key in front matter.** A page with `version: v2` moves
   under that version's entry, keeping its relative path.

Releasing v4 means setting *Latest version* to `v4` and adding `v3 =
docs-v3`; the next sync moves the pages.

## Limits

512 KiB per page, 2000 pages, nesting up to 8 levels, 12 versions. Images are
shown from public repositories only (their address is the raw file at the
commit). Footnotes and math are left as literal text.

## Troubleshooting

The Plugins page shows the `sync` task's last status and error.

- **404**: the repository, branch or folder is wrong, or the token cannot
  see the repository.
- **403**: GitHub's rate limit. Add a token (it raises the limit).
- **401**: the token is wrong or expired.
- A sync that cannot finish in one minute pauses and continues on the next
  run; nothing is lost.

## Building

```bash
./build.sh           # builds docs-component.wasm and records the source hash
./build.sh --check   # fails when the committed component is not built from these sources
VYASA_SIGNING_KEY=<author secret> ./package.sh   # packs and prints the sha-256
```

The marketplace package is always built from source by `package.sh`; the
committed component is a test fixture.

## Licence

MIT OR Apache-2.0.
