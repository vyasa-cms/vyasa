# Forms

Defined forms collect sign-ups, enquiries and the like. A form has its own fields, an optional address that is
told about each entry, and a success message. Build one under **Forms** in the admin and put it on a page with the
form block.

## Fields

`text`, `email`, `textarea`, `number`, `select` (with choices) and `checkbox` (stored as yes or no). A form
that asks for a promise adds a `consent` field: a checkbox the sender must tick, which can link your privacy
page (an address starting with `/` or `https://`). The entry stores the answer and, as `<key>_consent_at`, the
time it was given.

## Hold entries until the sender confirms (double opt-in)

Turn on **Hold each entry until its sender confirms by email**. The form then needs a required `email`
field, and your site address must be set (Settings). What happens:

1. The sender submits. The entry is stored as *held*: it is left out of the inbox, the CSV and the unread counts.
2. A confirmation mail goes to the sender through the mail queue. Its link opens a page with a button, so a mail
   scanner that fetches the link changes nothing; the click confirms the entry.
3. Confirming makes the entry count, and mails the form's notification address once, naming only the form, the
   number of confirmed entries and a link to the admin. Names, addresses and answers stay in the admin.
4. Every mail to the sender carries a link that deletes the entry.
5. A held entry that is not confirmed within seven days is removed by the hourly job. A second submission from
   the same address replaces the held one.

The owner can list the held entries with `GET /api/v1/forms/{id}/submissions?held=true` (also on
`submissions.csv`).

The address check on every form and on newsletter sign-up refuses lists and angle brackets
(`a@x.co,b@y.co`, `Name <a@x.co>`), so one sign-up cannot address several recipients.
