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
