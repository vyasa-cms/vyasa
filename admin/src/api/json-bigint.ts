/**
 * JSON parsing that survives the server's 64-bit Snowflake ids.
 *
 * Ids are i64 values around 3.5e17. JavaScript numbers carry 53 bits of
 * integer precision (2^53 ≈ 9.0e15), so `JSON.parse` silently rounds them and
 * `String(id)` then produces a *different* number than the server stored:
 *
 *   stored     350710414700445696
 *   JSON.parse 350710414700445696   (value happens to round-trip)
 *   String(…)  "350710414700445700" (shortest form for that double)
 *   server     350710414700445700 → not found
 *
 * That made every "edit this post" link in the admin resolve to a row that
 * does not exist. The wire format is fine — i64 is perfectly ordinary for a
 * Rust, Go or Python client — so the fix belongs here, in the one consumer
 * that cannot represent it, rather than in the public API.
 *
 * Integers too large to be exact become strings; everything else parses as
 * usual.
 */

/** Beyond this magnitude an integer literal cannot survive a double. */
const MAX_SAFE = BigInt(Number.MAX_SAFE_INTEGER);

/**
 * Quotes unsafe integer literals so `JSON.parse` keeps their exact digits.
 *
 * Walks the text rather than using a global regex, because a naive pattern
 * would also rewrite digits that appear *inside* string values — a post whose
 * body mentions a long number, for instance.
 */
/** True when the literal cannot survive a round trip through a double. */
function isUnsafeInteger(literal: string): boolean {
  try {
    const value = BigInt(literal);
    return value > MAX_SAFE || value < -MAX_SAFE;
  } catch {
    return false;
  }
}

export function quoteUnsafeIntegers(text: string): string {
  let out = "";
  let i = 0;
  let inString = false;

  while (i < text.length) {
    const ch = text[i] ?? "";

    if (inString) {
      out += ch;
      if (ch === "\\") {
        // Copy the escaped character verbatim so an escaped quote does not
        // look like the end of the string.
        i += 1;
        out += text[i] ?? "";
      } else if (ch === '"') {
        inString = false;
      }
      i += 1;
      continue;
    }

    if (ch === '"') {
      inString = true;
      out += ch;
      i += 1;
      continue;
    }

    // A number can only start where a value is expected.
    const isNumberStart =
      (ch >= "0" && ch <= "9") ||
      (ch === "-" && (text[i + 1] ?? "") >= "0" && (text[i + 1] ?? "") <= "9");

    if (!isNumberStart) {
      out += ch;
      i += 1;
      continue;
    }

    let j = i;
    if (text[j] === "-") j += 1;
    while (j < text.length) {
      const c = text[j] ?? "";
      if (c >= "0" && c <= "9") j += 1;
      else break;
    }

    const next = text[j] ?? "";
    const isInteger = next !== "." && next !== "e" && next !== "E";
    const literal = text.slice(i, j);

    if (isInteger && literal !== "" && literal !== "-") {
      out += isUnsafeInteger(literal) ? `"${literal}"` : literal;
    } else {
      // A float or exponent — precision beyond a double is not expected, and
      // quoting one would change its type.
      let k = j;
      while (k < text.length && /[0-9eE+\-.]/.test(text[k] ?? "")) k += 1;
      out += text.slice(i, k);
      i = k;
      continue;
    }
    i = j;
  }

  return out;
}

/** `JSON.parse` that preserves oversized integers as strings. */
export function parseJsonPreservingIds<T>(text: string): T {
  return JSON.parse(quoteUnsafeIntegers(text)) as T;
}
