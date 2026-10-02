# MCP server — manage content from Claude

`vyasa mcp` speaks the Model Context Protocol over stdio, backed by the
running site's REST API. Every capability check, validation rule, rate
limit and audit row applies to the AI exactly as it would to a person
holding the same API key — the MCP server is a courier, not a back door.

## Setup

1. Create an API key in the admin (API keys), owned by a user whose role
   matches what you want the AI to be able to do (an `author` key cannot
   publish other people's work, exactly as in the admin).
2. Register the server with your MCP client. For Claude Code:

```bash
claude mcp add vyasa \
  --env VYASA_URL=https://your-site.example \
  --env VYASA_API_KEY=vy_... \
  -- /path/to/vyasa mcp
```

`VYASA_URL` defaults to `http://127.0.0.1:3000` for a local site.

## Tools

| Tool | What it does |
|---|---|
| `list_posts` | Entries by status (`draft`, `published`, …), paginated |
| `search_posts` | Full-text search over published content |
| `get_post` | One entry with its full content |
| `create_post` | New entry — plain text (`content_text`: `#` headings, ``` fences, blank-line paragraphs) or a full block document (`content_blocks`); drafts by default |
| `update_post` | Change title, content, excerpt or status |
| `publish_post` | Publish now |
| `trash_post` | Move to trash (restorable in the admin) |

Plain text is escaped on the way in: pasted markup becomes visible text,
never live HTML. For rich layouts, send `content_blocks` in the same
`{schema_version: 1, blocks: [...]}` shape the editor stores.
