/**
 * How an error becomes something a person reads.
 *
 * Messages arrive from several places — our own API, a provider quoted by
 * it, a thrown `Error`, a rejected fetch — and they are not all short. One
 * rate limit from OpenRouter arrived as three hundred characters of nested
 * JSON and every surface that showed it ran off the edge of its panel or
 * cut off mid-word. The server no longer sends those, but nothing about a
 * layout should depend on that staying true.
 */

/** The readable text of whatever was thrown, on one line. */
export function errorText(error: unknown): string {
  const raw =
    error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : String(error);
  // Newlines and runs of spaces come from JSON bodies and stack traces and
  // only ever make the first line look broken.
  return raw.replace(/\s+/g, " ").trim();
}

/** Where to stop showing a message before it stops being read. */
const READABLE = 180;

/**
 * Splits a message into the part worth showing and the rest.
 *
 * Cuts on a sentence if there is one within reach, otherwise on a word, so
 * the visible half always ends somewhere deliberate. `rest` is the full
 * text, not the tail: whoever opens the details wants the whole thing, not
 * to reassemble it.
 */
export function errorSummary(error: unknown): { summary: string; rest: string | null } {
  const text = errorText(error);
  if (text.length <= READABLE) return { summary: text, rest: null };

  const window = text.slice(0, READABLE);
  const sentence = Math.max(window.lastIndexOf(". "), window.lastIndexOf("? "));
  const cut = sentence > READABLE / 2 ? sentence + 1 : window.lastIndexOf(" ");
  const clipped = cut > 0 ? window.slice(0, cut) : window;
  // A cut that lands on a separator leaves "…response body |…", which reads
  // as though something failed to render rather than as an abbreviation.
  const summary = clipped.replace(/[\s|,;:·–—-]+$/, "");
  return { summary: `${summary}…`, rest: text };
}
