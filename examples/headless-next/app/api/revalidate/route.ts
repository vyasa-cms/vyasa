import { revalidatePath } from "next/cache";
import { createHmac, timingSafeEqual } from "node:crypto";

/**
 * Webhook receiver: rebuilds affected pages when content changes in Vyasa.
 *
 * Without this, `revalidate: 60` is the only freshness mechanism and an
 * editor waits up to a minute to see a correction. With it, publishing is
 * reflected immediately and the time-based revalidation becomes a backstop.
 *
 * Point a Vyasa webhook at `POST /api/revalidate` subscribed to
 * post.published, post.updated and post.trashed, and put its secret in
 * VYASA_WEBHOOK_SECRET.
 */

const SECRET = process.env.VYASA_WEBHOOK_SECRET;

/** Vyasa's header: `t=<unix>,v1=<hex hmac-sha256(secret, "<t>.<body>")>`. */
const SIGNATURE_HEADER = "x-vyasa-signature";

/** Reject anything older than this, so a captured request cannot be replayed. */
const MAX_AGE_SECONDS = 300;

function verify(signature: string, body: string): boolean {
  if (!SECRET) return false;

  let timestamp: string | undefined;
  let received: string | undefined;
  for (const part of signature.split(",")) {
    const [key, value] = part.trim().split("=");
    if (key === "t") timestamp = value;
    if (key === "v1") received = value;
  }
  if (!timestamp || !received) return false;

  const age = Math.floor(Date.now() / 1000) - Number(timestamp);
  if (!Number.isFinite(age) || age > MAX_AGE_SECONDS) return false;

  const expected = createHmac("sha256", SECRET)
    .update(`${timestamp}.${body}`)
    .digest("hex");

  // Compare in constant time: a byte-by-byte early exit leaks the signature
  // one character at a time.
  const a = Buffer.from(expected, "utf8");
  const b = Buffer.from(received, "utf8");
  return a.length === b.length && timingSafeEqual(a, b);
}

export async function POST(request: Request): Promise<Response> {
  if (!SECRET) {
    // Failing closed: an unconfigured receiver that accepted everything
    // would be an open cache-invalidation endpoint.
    return Response.json(
      { error: "VYASA_WEBHOOK_SECRET is not set" },
      { status: 503 },
    );
  }

  const signature = request.headers.get(SIGNATURE_HEADER);
  // Read the raw body: re-serialising the parsed JSON would reorder keys and
  // invalidate the signature.
  const body = await request.text();

  if (!signature || !verify(signature, body)) {
    return Response.json({ error: "bad signature" }, { status: 401 });
  }

  const payload = JSON.parse(body) as {
    event: string;
    data?: { post_id?: number };
  };

  // The payload carries an id, not a slug, so the specific page cannot be
  // targeted without a lookup. Revalidating the list plus the post segment
  // covers it in two calls.
  revalidatePath("/");
  revalidatePath("/posts/[slug]", "page");

  return Response.json({ revalidated: true, event: payload.event });
}
