/**
 * A minimal typed client for Vyasa's GraphQL API.
 *
 * GraphQL rather than REST because a listing page needs the post plus its
 * terms and author in one round trip; the REST list would need a follow-up
 * request per post.
 */

const VYASA_URL = process.env.VYASA_URL ?? "http://localhost:3000";
const API_KEY = process.env.VYASA_API_KEY;

export interface Post {
  /**
   * Snowflake id. GraphQL sends it as a JSON number, and JavaScript carries
   * only 53 bits of integer precision, so this value can come back subtly
   * wrong. Key your React lists and your routes on `slug`, not on this.
   */
  id: number;
  slug: string;
  title: string;
  excerpt: string | null;
  publishedAt: string | null;
  content: unknown;
}

interface GraphQLResponse<T> {
  data?: T;
  errors?: { message: string }[];
}

async function query<T>(
  document: string,
  variables: Record<string, unknown> = {},
): Promise<T> {
  const headers: Record<string, string> = {
    "content-type": "application/json",
  };
  // Published content is public; a key is only needed to see drafts.
  if (API_KEY) headers.authorization = `Bearer ${API_KEY}`;

  const response = await fetch(`${VYASA_URL}/api/graphql`, {
    method: "POST",
    headers,
    body: JSON.stringify({ query: document, variables }),
    // Vyasa's own render cache sits in front of the public site, not the
    // API, so caching decisions belong to the consumer.
    next: { revalidate: 60 },
  });

  if (!response.ok) {
    throw new Error(`Vyasa returned ${response.status} ${response.statusText}`);
  }

  const body = (await response.json()) as GraphQLResponse<T>;
  if (body.errors?.length) {
    // Surfacing only the first message keeps the failure readable; the rest
    // are almost always the same problem reported per field.
    throw new Error(`Vyasa GraphQL error: ${body.errors[0].message}`);
  }
  if (!body.data) throw new Error("Vyasa returned no data");
  return body.data;
}

const POST_FIELDS = `
  id
  slug
  title
  excerpt
  publishedAt
`;

export async function listPosts(first = 10): Promise<Post[]> {
  const data = await query<{
    posts: { edges: { node: Post }[] };
  }>(
    `query ListPosts($first: Int!) {
       posts(first: $first, status: "published", postType: "post") {
         edges { node { ${POST_FIELDS} } }
       }
     }`,
    { first },
  );
  return data.posts.edges.map((edge) => edge.node);
}

export async function getPostBySlug(slug: string): Promise<Post | null> {
  const data = await query<{ postBySlug: Post | null }>(
    `query GetPost($slug: String!) {
       postBySlug(slug: $slug, postType: "post") {
         ${POST_FIELDS}
         content
       }
     }`,
    { slug },
  );
  return data.postBySlug;
}

/**
 * Posts carrying a given term, looked up by the term's slug.
 *
 * Two round trips, because the schema exposes `terms` (a full list) and
 * `posts(termId:)` but no `termBySlug`. For a site with few tags the list is
 * small and cacheable; if yours has thousands, add a `termBySlug` query
 * server-side rather than paying this on every request.
 */
export async function listPostsByTerm(slug: string, first = 20): Promise<Post[]> {
  const terms = await query<{
    terms: { id: number; slug: string }[];
  }>(`query Terms { terms { id slug } }`);

  const term = terms.terms.find((t) => t.slug === slug);
  if (!term) return [];

  const data = await query<{ posts: { edges: { node: Post }[] } }>(
    `query PostsByTerm($termId: Int!, $first: Int!) {
       posts(termId: $termId, first: $first, status: "published", postType: "post") {
         edges { node { ${POST_FIELDS} } }
       }
     }`,
    { termId: term.id, first },
  );
  return data.posts.edges.map((edge) => edge.node);
}

/**
 * Renders Vyasa's block document as plain HTML.
 *
 * Deliberately minimal: it handles the text blocks a headless frontend
 * usually wants and ignores the rest, rather than half-implementing the
 * whole block vocabulary. Extend it for the blocks your site actually uses.
 */
export function renderBlocks(content: unknown): string {
  const doc = content as { blocks?: Block[] } | null;
  if (!doc?.blocks) return "";
  return doc.blocks.map(renderBlock).join("\n");
}

interface Block {
  kind: string;
  attrs?: Record<string, unknown>;
  children?: Block[];
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function renderBlock(block: Block): string {
  const text = escapeHtml(String(block.attrs?.text ?? ""));
  switch (block.kind) {
    case "paragraph":
      return `<p>${text}</p>`;
    case "heading": {
      const level = Number(block.attrs?.level ?? 2);
      const h = Math.min(Math.max(level, 1), 6);
      return `<h${h}>${text}</h${h}>`;
    }
    case "quote":
      return `<blockquote>${text}</blockquote>`;
    case "code":
      return `<pre><code>${text}</code></pre>`;
    default:
      // An unknown block is skipped rather than guessed at: emitting the
      // raw attrs would leak internal shape into the page.
      return "";
  }
}
