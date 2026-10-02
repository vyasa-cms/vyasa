// Generates the REST API reference (content/docs/api) from the server's
// OpenAPI document, which CI keeps in sync with the code.
import { rm } from 'node:fs/promises';
import { generateFiles } from 'fumadocs-openapi';
import { createOpenAPI } from 'fumadocs-openapi/server';

await rm('./content/docs/api', { recursive: true, force: true });
const openapi = createOpenAPI({ input: ['../admin/openapi.json'] });
// One page per endpoint, in folders by route: operation ids repeat across
// tags (several handlers are called `create`), routes never do.
await generateFiles({ input: openapi, output: './content/docs/api', per: 'operation', groupBy: 'route', includeDescription: true });

// The section's own landing page and its name in the sidebar.
const { writeFile } = await import('node:fs/promises');
await writeFile(
  './content/docs/api/index.mdx',
  `---
title: REST API
description: Every endpoint of /api/v1, generated from the server's OpenAPI document.
---

Vyasa's REST API lives under \`/api/v1\`. Sign in with a session cookie (\`POST /api/v1/auth/login\`) or send an API key as \`Authorization: Bearer <key>\`. Who may call each route is listed in [route access](/docs/reference/route-access).

The pages in this section are generated from \`admin/openapi.json\`, which the server also serves at \`/api/openapi.json\` and \`/api/docs\` (Swagger UI) on every install.
`,
);
await writeFile('./content/docs/api/meta.json', JSON.stringify({ title: 'REST API', pages: ['index', '...'] }, null, 2));
