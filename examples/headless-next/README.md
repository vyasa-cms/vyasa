# Headless Vyasa with Next.js

A minimal Next.js App Router frontend that reads posts from a Vyasa instance
over GraphQL. It exists to show the shape of a headless integration, not to
be a starter theme.

## Running it

```bash
cp .env.example .env.local     # point VYASA_URL at your instance
pnpm install
pnpm dev
```

With a Vyasa server on `http://localhost:3000` and some content
(`vyasa dev seed --count 20`), the list and post pages work immediately —
published content needs no credentials.

## What it demonstrates

- **GraphQL over REST for reads.** One request returns a post with its
  author and terms; the REST list would need a follow-up call per post.
- **Reading without a key.** Published posts are public. An API key is only
  needed for drafts, previews or anything a logged-out visitor cannot see.
- **Rendering block content.** `renderBlocks` in `lib/vyasa.ts` turns
  Vyasa's block document into HTML for the four text blocks a headless
  frontend usually wants, escaping every value. Extend it for the blocks
  your site actually uses.

## Instant updates via webhook

`app/api/revalidate/route.ts` receives Vyasa webhooks and calls
`revalidatePath`, so publishing shows up immediately instead of waiting out
the 60-second revalidation window. It verifies the `X-Vyasa-Signature`
HMAC in constant time, rejects anything older than five minutes, and fails
closed when no secret is configured.

Create the webhook in the admin (Site → Webhooks) pointed at
`https://your-frontend/api/revalidate`, subscribed to `post.published`,
`post.updated` and `post.trashed`. Put the secret it shows you in
`VYASA_WEBHOOK_SECRET`.

## Two things that will bite you

**Post ids are 64-bit.** GraphQL sends them as JSON numbers and JavaScript
carries 53 bits, so `id` can come back subtly wrong. Key lists and routes on
`slug`. (Vyasa's own REST API sends ids as strings for this reason; the
GraphQL schema does not.)

**Caching is yours to decide.** Vyasa's render cache sits in front of its own
public site, not in front of the API, so every API request does real work.
This example sets `next: { revalidate: 60 }`; pick a number that matches how
often your content changes.

## Getting an API key

Only needed for non-public content:

```bash
curl -X POST "$VYASA_URL/api/v1/api-keys" \
  -H 'content-type: application/json' \
  -b "vy_session=$SESSION" \
  -d '{"name":"next frontend","capabilities":["edit_posts"]}'
```

The key is shown once. A key can never grant more than the user who created
it holds.
