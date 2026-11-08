# Documentation sites

Vyasa serves landing pages (theme sections), blogs (posts) and developer
documentation. Documentation is a content type, `doc`, that nests: pages
have parents, a position among their siblings, and addresses such as
`/docs/guides/install`. The `vyasa-docs` plugin fills it from a folder of
Markdown in a GitHub repository, and supplies the sidebar, the contents,
previous and next links, versions, search and the blocks that documentation
needs. This site is built that way, from the `docs/` folder of the Vyasa
repository.

## Set it up

1. Install **Docs** from Plugins → Browse plugins and enable it. It asks for
   the capabilities it needs (fetching from GitHub, writing the pages it
   imports, running a background task for up to a minute); nothing else.
2. Open its settings (Plugins → Docs). Set the **repository**
   (`https://github.com/owner/name`), the **branch** and the **folder** that
   holds the Markdown. A private repository needs an **access token**: a
   fine-grained token with read-only access to that repository's Contents.
3. Within a minute the pages appear under `/docs`. Pages that come from the
   repository are read-only in the editor and show an *Edit source* link;
   edit them on GitHub and they follow.
4. For instant updates add a webhook to the repository: payload URL
   `https://your-site/api/v1/plugin/vyasa-docs/webhook`, content type
   `application/json`, the same secret as the plugin's **webhook secret**,
   "Just the push event". Without it the plugin checks every hour (a check
   with nothing new is one request).

## Write the pages

Plain Markdown. The first `# Heading` is the page title. Optional front
matter between `---` lines:

```yaml
---
title: Installing
description: One sentence for search results and link previews.
position: 2          # order among siblings; also: order, sidebar_position, weight
slug: install        # default: the file name
section: Basics      # a heading shown above this page in the sidebar
hide: true           # keep it out of the sidebar and the pager
version: v1          # file this page under an older version
---
```

Folders become sections. A folder's `index.md` (or `README.md`) is the
section's own page; without one a list of its pages is generated. Order is
decided by a `_meta.json` in the folder, either
`{"title": "Guides", "pages": ["install", "deploy"]}` or an ordered object
`{"install": "Installing", "secret": {"title": "Secret", "hidden": true}}`;
then by `position`; then by name. A numeric file name prefix (`02-deploy.md`)
also orders, and is dropped from the address.

Relative links between pages work; links to other files in the repository
become links to GitHub at the synced commit. Images need the repository to
be public.

### Components

Seven tags, at the start of a line, are understood; everything else is
Markdown. No JavaScript, no imports.

```mdx
<Callout type="warning" title="Heads up">
Types: info, tip, warning, danger.
</Callout>

<Tabs items={["npm", "pnpm"]}>
<Tab>

    npm install vyasa

</Tab>
<Tab>

    pnpm add vyasa

</Tab>
</Tabs>

<Steps>

### Install

Do this first.

### Configure

Then this.

</Steps>

<Params>
<Param name="folder" type="string" default="docs">Where the Markdown lives.</Param>
<Param name="token" type="string" required>Only for private repositories.</Param>
</Params>
```

Also understood: GitHub alerts (`> [!NOTE]`), fenced code blocks that share
`group="install"` become a tabbed code group (the tab is named by `title="…"`),
and a fence with the language `tree` becomes a file tree (two spaces per
level, a trailing `/` marks a folder).

## Versions

Serve the newest version at `/docs` and older ones at `/docs/v1/…`. In the
plugin's settings set **Latest version label** (`v2`) and list the older ones,
one per line, as `label = folder` (`v1 = docs-v1`); or put `version: v1` in a
page's front matter. Pages of an older version show a notice with a link to
the same page in the latest version, and search from `/docs` is limited to
the version being read. To release v3: set the latest label to `v3`, add
`v2 = docs-v2`, and push.

## Search, `llms.txt` and Markdown

The search box submits to the site's own search, limited to documentation
and to the version being read (`/search?type=doc&term=doc-version:v2`). Every
page is also served as Markdown at its address plus `.md`, and listed in
`/llms.txt`, for tools that read documentation rather than render it.

## Behind the scenes

The same pieces are available to any plugin: content types that nest
(`hierarchical`), an address base apart from the type's name, a layout tree
per type in a theme's `layout.json`, search limited by type and term, and
*managed entries* (pages a plugin keeps in step with an outside source). See
[the plugin API](plugin-api.md) and [themes](THEMES.md#typed-layouts).
