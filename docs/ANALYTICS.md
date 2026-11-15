# Audience analytics

Vyasa counts visits on its own server. There is no tracking script, no cookie and no stored IP address, and only
aggregates exist: day counters and a count of unique visitors.

## What is counted

A `GET` that leaves with an HTML `200` on a public path (not `/admin`, `/api`, `/preview`, `/metrics`) moves:

| Counter | Meaning |
|---|---|
| views | per day, path and referring host (a different site only) |
| visitors | unique visitors per day |
| bots | requests whose user agent announces a crawler or tool (or has none); not part of the views |
| device, browser | class (`desktop`, `mobile`, `tablet`) and family from the user agent, as counters only |
| `utm_source`, `utm_medium`, `utm_campaign` | the values of the query string, lower-cased, at most 60 characters |
| entry pages | views with no same-site referrer |
| 404s | the missing path and the referring host |
| searches | internal `/search?s=` terms |
| outbound | clicks through `/go/<name>` |
| conversions | a form or newsletter signup, and `/go/` links marked as one |

## What is never counted

- Requests with `X-Krishiv-Internal: 1` (link checks, staging QA, our own tools).
- Requests with `DNT: 1` or `Sec-GPC: 1`.
- Anyone holding a session cookie (a signed-in author or admin).

## Unique visitors without an identifier

A visitor is `sha256(day secret, client address, user agent, host)`. The day secret is random, lives in the process's
memory for one UTC day and is never written anywhere. The day's hashes are kept only to count each visitor once, and
they are deleted when the next day's first visit arrives. No identifier outlives its day. A restart in the middle of
a day starts a new secret, so a visitor can then count twice that day; never across days. The address is read only
to make the hash (an IPv6 address by its /64) and is not stored.

There are no raw request logs of visits: the counters are the only record, so there is nothing to expire.

## Outbound links

`/go/<name>` redirects to the target in the `audience_links` table (managed on the Audience page, or
`PUT /api/v1/analytics/links/{name}`) and counts the click. A name is 1 to 64 characters of `a-z`, `0-9`, `-`, `_`;
a target is an absolute `http` or `https` URL. A link marked `conversion` also counts as a conversion.

## Text for a privacy page

> We count visits on our server. No cookies, no tracking scripts, no personal data stored.

## Not yet

Country (needs a GeoIP database file, see the open question in the release notes) and Search Console import (A7).
