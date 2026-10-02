// Copies the repository's Markdown docs into content/docs so the website
// and the repository never disagree: docs are written once, in docs/, and
// this script gives each page frontmatter and rewrites its links.
//
// Generated files are git-ignored; run by `pnpm sync:docs` (and prebuild).
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, join, posix } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, '..', '..');
const out = join(here, '..', 'content', 'docs');
const github = 'https://github.com/vyasa-cms/vyasa/blob/main';

/** Repository file -> site page (slug under /docs). */
const pages = {
  'docs/FEATURE-MATRIX.md': 'features',
  'docs/ARCHITECTURE.md': 'architecture',
  'docs/AI-MODELS.md': 'using/ai-models',
  'docs/BLOCKS.md': 'building/blocks',
  'docs/THEMES.md': 'building/themes',
  'docs/plugin-api.md': 'building/plugins',
  'docs/MARKETPLACE.md': 'building/marketplace',
  'docs/MCP.md': 'building/mcp',
  'docs/DEPLOYMENT.md': 'running/deployment',
  'docs/OPERATIONS.md': 'running/operations',
  'docs/SECURITY.md': 'running/security',
  'docs/VERSIONING.md': 'running/versioning',
  'docs/PERFORMANCE.md': 'running/performance',
  'docs/ROUTE-ACCESS.md': 'reference/route-access',
  'CONTRIBUTING.md': 'contributing',
};

const generated = new Set(['features', 'architecture', 'using', 'building', 'running', 'reference', 'contributing']);

function rewriteLinks(markdown, from) {
  return markdown.replace(/\]\(([^)\s]+)\)/g, (whole, target) => {
    if (/^[a-z]+:|^#|^\//i.test(target)) return whole;
    const [path, anchor] = target.split('#');
    const resolved = posix.normalize(posix.join(posix.dirname(from), path));
    const page = pages[resolved];
    const suffix = anchor ? `#${anchor}` : '';
    if (page) return `](/docs/${page}${suffix})`;
    return `](${github}/${resolved}${suffix})`;
  });
}

function split(markdown) {
  const lines = markdown.split('\n');
  const first = lines.findIndex((l) => l.startsWith('# '));
  if (first === -1) return { title: 'Untitled', body: markdown };
  const title = lines[first].slice(2).trim();
  const body = [...lines.slice(0, first), ...lines.slice(first + 1)].join('\n').trimStart();
  return { title, body };
}

for (const dir of generated) await rm(join(out, dir), { recursive: true, force: true });
await rm(join(out, 'features.md'), { force: true });
await rm(join(out, 'architecture.md'), { force: true });
await rm(join(out, 'contributing.md'), { force: true });

for (const [from, slug] of Object.entries(pages)) {
  const source = await readFile(join(repo, from), 'utf8');
  const { title, body } = split(source);
  const file = join(out, `${slug}.md`);
  await mkdir(dirname(file), { recursive: true });
  const front = `---\ntitle: ${JSON.stringify(title)}\n---\n\n`;
  const note = `<!-- Generated from ${from} by scripts/sync-docs.mjs. Edit that file instead. -->\n\n`;
  await writeFile(file, front + note + rewriteLinks(body, from));
}
console.log(`synced ${Object.keys(pages).length} pages`);
