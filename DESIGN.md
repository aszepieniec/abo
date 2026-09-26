# abo — accountant back office

Bookkeeping for a small company. A git repository holds the books; a CLI
operates on it; a web server wraps the same functions in a browser interface; an
AI proposes bookings from scanned documents which a human approves.

This document exists so that work can resume on a different machine with a
different agent. It records what was decided and **why**, so the decisions are
not re-litigated. Where something was deliberately left out, the trigger for
adding it is stated.

Status: M0-M4 built — `abo init`, `add`, `bill`, `invoice`, `reconcile`,
`clear`, `pl`, `bs`, `vat`, `bal`, `reg`, `serve`, `demo`. Next step is M5
(companies and users).

---

## 1. Decisions locked

| Decision | Choice | Why |
|---|---|---|
| Language | Rust | Stated requirement. |
| Storage | The git repository itself. No database. | Audit trail, versioning, undo, backup and sync become `git log`, reverting a commit, `git push`. Approve = commit. |
| Journal format | hledger | Follows from the reports decision below. |
| Reports | Minimal Rust parser for writing and validating; **hledger computes everything**. | Multi-commodity, elided amounts, price directives and period expressions are where a hand-rolled money parser goes quietly wrong. hledger is already installed (GPL-3+, shelled out, no licence obligation — see §9). |
| Invoice PDF | typst template, `typst compile` shelled out | One page, fixed grid. Layout lives in a `.typ` template, not in Rust. Compiler is Apache-2.0, offline, no account. |
| Git access | shell out to `git` | Smaller than `git2`/libgit2 by a wide margin. |
| Web UI | axum + maud, server-rendered, one hand-written stylesheet | No SPA, no npm, no build step, no WASM. HTML escaping comes from maud. One hand-written stylesheet, not a framework — see §7. |
| Deployment | localhost first, reachable later | See §8. Auth goes at a reverse proxy, not in the app — until users exist. |
| Tax | three scheme *shapes* in code (`none` / `flat` / `effective`); every rate, rule and name in config | The shape changes what a *correct booking* is, so the validator must know it. Rates and local rules differ everywhere and are data. See §6. |
| Commodities | multi-commodity from the start | hledger already is. Cash, bank and wallet balances are all just asset accounts. |
| Phone app | PWA over the existing server-rendered pages | See §7. |

### Crates

`clap serde toml serde_json axum tokio maud reqwest base64 rust_decimal jiff sha2 anyhow`

One tokio runtime; everything async. No ORM, no template engine, no JS toolchain.

---

## 2. Repository layout (the books)

```
company-books/
  abo.toml
  journal/
    main.journal          # `include`s the rest
    accounts.journal      # account + commodity declarations
    2026.journal          # one file per year
  templates/invoice.typ   # seeded on first use, then yours to edit
  bills/2026/2026-09-21-acme-a1b2c3d4.pdf
  invoices/2026/2026-0017.pdf
  .abo/                   # gitignored: bills awaiting a decision, their threads
```

Documents are linked to entries by hledger tags, never by a join table:

```
2026-09-21 Acme Hosting
    ; bill: bills/2026/2026-09-21-acme-inv1234.pdf
    ; sha256: 9f2a...
    expenses:hosting        100.00 EUR
    assets:vat:input          10.00 EUR
    liabilities:payable    -1010.00 EUR
```

- `sha256:` is the duplicate check. Before booking a document, `hledger print tag:sha256=<hash>` must return nothing.
- `invoice:` on issued invoices carries the invoice number. The next number is
  `max(invoice:) + 1` read from the journal — no counter file, no state to corrupt.

Account naming: `assets:bank`, `expenses:telecom`. If your accountant wants a
numbered chart of accounts, `assets:1020:bank` also sorts correctly.

---

## 3. Source layout

```
src/main.rs      clap dispatch, #[tokio::main]
src/repo.rs      abo.toml, path resolution, git add/commit
src/journal.rs   read account declarations, build + validate + append entries
src/ai.rs        one POST to /v1/messages
src/report.rs    hledger passthrough (-O json / -O csv)
src/invoice.rs   fill typst template, compile
src/web.rs       axum + maud
```

Ten files plus two typst templates (`bill.rs` was split out of the plan's seven so
that `ai.rs` stays pure transport and can be swapped or stubbed without touching
the workflow). No `lib.rs`/`bin/` split, no workspace, no crate-per-module.

### Three invariants that keep future work cheap

These are free now and expensive to retrofit. Do not violate them.

1. **Every workflow function takes a context first.** `propose_bill(&ctx, path)`,
   never `propose_bill(path)` reading a global. Multi-company then means building
   a different `Ctx`; users mean `Ctx` carries an actor. Authorization has exactly
   one place to live.
2. **No rendering below the dispatch layer.** No HTML, no terminal formatting, no
   `println!` inside workflow functions. They return serde structs.
3. **`--json` on the CLI is the API schema.** The struct a phone client would
   receive is the struct the CLI already prints, so it is exercised from day one.
   A JSON API later is a second axum router over unchanged functions.

The web server calls these functions **in-process**. It does not shell out to the
CLI and it does not talk to itself over HTTP.

---

## 4. The bill workflow (the core)

1. `abo bill <file>` — hash the file, check `sha256:` for a duplicate.
2. Build the request (§5), send, receive a structured proposal.
3. Validate in Rust (§6). The model proposes; it never decides.
4. Show the rendered entry. User approves, or types a correction.
5. On correction: append the user's text to the message thread, re-request.
   The thread is persisted as JSON keyed by the file's sha256, so the CLI and the
   browser resume the same conversation.
6. On approval: copy the document to its final path, `git add` it, then append
   the entry — the commit inside `add` sweeps up the index, so the document and
   its entry land in one commit or not at all. If the entry fails at that point
   the copy is removed and the path reset.

The destination is `bills/<year>/<date>-<vendor-slug>-<sha8>.<ext>`. The vendor
slug keeps only `[a-z0-9]` and collapses the rest to `-`, so a vendor name of
`../../.ssh/authorized_keys` cannot escape the directory. There is a test for
exactly that. The supplier's own invoice number is tagged `ref:`, never
`invoice:`, which is reserved for the numbering of invoices we issue.

Document filenames are derived from the sha256 and a sanitized vendor string —
**never** from a client-supplied filename (§8).

---

## 5. The AI call

Rust has no official Anthropic SDK, so this is raw HTTP with `reqwest` +
`serde_json`. About 40 lines total.

- `POST https://api.anthropic.com/v1/messages`, `x-api-key` from `ANTHROPIC_API_KEY`.
- Model `claude-opus-5`. Thinking is on by default on this model; `output_config`
  `effort` of `medium` is the starting point.
- **System block** (stable — mark it with `cache_control` `ephemeral` so it caches):
  company details from `abo.toml`, the VAT rules for the configured method, the
  output of `hledger accounts`, and `hledger print` of recent entries for this
  vendor if any exist.
- **User block**: the document as a base64 `document` content block (PDF) or
  `image` block, placed *before* the text instruction.
- **Output**: a single structured result. Not an agent loop — the model has no
  filesystem access and no tools that touch the repository.

> An agent resuming here should load the bundled `claude-api` skill (or
> `curl/examples.md` within it) to confirm the exact current request shape for
> structured output and document blocks before writing `ai.rs`. The API surface
> moves; the schema below is ours and does not.

Proposal schema:

```json
{
  "date": "2026-09-21",
  "vendor": "Acme Hosting GmbH",
  "description": "Hosting September 2026",
  "currency": "EUR",
  "postings": [{"account": "expenses:hosting", "amount": "100.00"}],
  "invoice_ref": "INV-1234",
  "confidence": "high",
  "questions": []
}
```

`questions` is how the model asks rather than guesses — a non-empty list blocks
auto-approval even under `--yes`.

As built: `output_config` carries both `effort` (`medium`) and
`format: {type: "json_schema", schema}`, which is what makes the response a
single text block of schema-valid JSON — `additionalProperties: false` and a
complete `required` are mandatory there. Thinking is on by default on
`claude-opus-5`, so the `thinking` parameter is omitted entirely. Amounts cross
the wire as **decimal strings**: a JSON number is an `f64` by the time serde
sees it. `stop_reason` is checked for `refusal` and `max_tokens` before the
body is parsed.

The conversation lives in `.abo/threads/<sha256>.json` and the document in
`.abo/pending/<sha256>.<ext>`, both gitignored — working state, not books. The
thread stores only text turns; the document is re-attached from disk on every
request, so one copy of the bytes exists and the thread stays small. Re-running
`abo bill` on the same file resumes that conversation instead of paying for a
fresh proposal. Nothing is written until a call succeeds.

`ABO_STAND_IN=<file>` replaces the call: every request returns the proposal in
that file, so the bill workflow can be exercised with no key and no network,
and a tester edits the file to play the model's next answer. Everything after
the call — validation, filing, the commit — is the real code.

Server-side refusal fallbacks (`fallbacks` + its beta header) are deliberately
not enabled: silently re-running a booking on a different model is not a
property this app should have. A refusal is an error the human sees.

---

## 6. Validation, money and tax

### The validation chain

Runs in `journal.rs`, on every entry from any source. Order matters; stop at the
first failure.

0. No field carries journal syntax (control characters, `;`, oversize).
1. Postings balance per commodity (or exactly one amount elided).
2. Every account already declared in `accounts.journal`. An undeclared account is
   an explicit "create this account?" step, never an implicit write.
3. VAT shape matches the configured method (below).
4. `sha256:` not already present in the journal.
5. Date inside an open period.
6. Write to a temporary journal, run
   `hledger check accounts balanced ordereddates`, and append + commit only on
   exit 0.

Step 6 is free and catches everything the first five miss. It is also the test.

Money is `rust_decimal::Decimal`, or `i64` minor units tagged with a commodity.
**Never `f64`.** Rates may be `Decimal`. Amounts cross the AI boundary as
decimal *strings*, because a JSON number is an `f64` by the time serde sees it.

### Commodities

The books are multi-commodity from the start, because hledger is. A posting
carries an amount and a commodity, the validator balances each commodity
separately, and nothing anywhere assumes there is only one. Bank balances, notes
in a drawer and a wallet are all just asset accounts:

```
assets:bank      assets:cash      assets:wallet:btc
```

`abo init` seeds `assets:bank` and `assets:cash`; anything else is an `account`
line in `accounts.journal`, and a second commodity is a `commodity` line. An
invoice can be denominated in any of them (`abo invoice --currency BTC`), and
`[invoice] decimals` — or `--decimals` — says how many places that commodity is
stated in: two for most money, eight for a wallet. Rounding happens in exactly
one place, `invoice::round`, half away from zero.

A payment option's details are free text for the same reason: bank details,
a wallet address, "cash on collection". abo never parses them, it only prints
them. The option also has an id for commands, a name for invoices and the
account the money arrives in; `abo payment-options` adds, edits and removes
options.

Amounts are never converted between commodities. A bill in one currency against
books in another is booked as stated and the model is told to say so. Valuing a
report in a single commodity is `hledger -X`, at the point of reporting, where
the rate is a reporting choice rather than a fact about the transaction.

### Reconciliation

Agreeing the books with what a bank, a cash box or a wallet actually holds is
manual, and it needs no state of its own: hledger already says a posting is
unmarked, pending or cleared, so marking one cleared means writing a `*` in
front of its account in the journal, which is a commit like any other. It is
the posting that is marked, not the transaction: a transfer from the bank to
the cash box is seen by two statements, often on different days, and ticking
it off against one must leave it outstanding against the other. A `*` on the
transaction line, as older books may have, still counts for all its postings.

```
abo reconcile --account assets:bank --date 2026-04-30 --balance 24390.00
abo clear 2026.journal:11:2026-02-28 ...
abo reconcile ... --record
```

`reconcile` prints three numbers and a list: what the books say has cleared by
that date, what the statement says, the difference, and every booked item not
yet ticked off. Ticking items until the difference is zero *is* the
reconciliation. The browser shows the same thing with checkboxes.

Two details that are easy to get wrong:

- **A key is `file:line:date:account`.** Line numbers come from hledger's own
  `tsourcepos`, and entries are written in date order, so a booking made
  between listing and marking shifts the lines below it. Carrying the date
  means a stale key is refused instead of silently marking the wrong
  transaction. Keys also arrive from a browser form, so the file part is
  checked to be a plain `*.journal` name inside `journal/`. The account comes
  last because it has colons of its own, and names which of the transaction's
  postings to mark.
- **`--record` asserts only when nothing is outstanding.** hledger's balance
  assertions are computed over every posting regardless of status, so there is
  no way to assert a cleared-only balance. With nothing outstanding the two
  numbers are the same and the assertion is exactly true, so it is written and
  hledger re-checks it on every read from then on — an edit that changes
  history before that date stops the books loading. With something still
  outstanding the statement figure is *not* the account balance, so the entry
  is recorded with its tags and no assertion rather than asserting something
  untrue.

Entries are inserted in date order rather than appended, which is what makes a
late-arriving bill ordinary rather than a failure of `hledger check
ordereddates`. That applies to every write, not just this one.

### Statements

`abo pl`, `abo bs`, `abo vat`, each with `--json` and `--pdf <file>`, and each a
page under `/report/`. Everything is computed by `hledger balance` over a query,
read back through the one JSON shape `reconcile.rs` already parses — mantissa
and scale, never the `floatingPoint` field beside them. A P&L is `balance
income` and `balance expenses` over a period; a balance sheet is the same three
queries with `-H` and an end date. There is no second report engine.

Two things are decided here rather than by hledger:

- **Which side is turned over.** Income, liabilities and equity carry credit
  balances. A reader wants both columns positive, so those sections are negated
  for display and the arithmetic is done on the signed figures underneath.
- **Unclosed profit is equity.** A balance sheet drawn from a journal does not
  balance until the year's result is counted, because nothing has closed income
  and expenses into equity. So the equity section carries a computed
  "Retained earnings, not yet closed" line, and the bottom line —
  assets less liabilities and equity — is nil. Without it the difference looks
  like a bug rather than the year's profit.

PDFs follow M3: abo writes `.abo/report.json`, a typst template lays it out,
and the template does no arithmetic. Unlike the invoice's, the report template
is not seeded into the books: a PDF is asked for with a GET — a download link,
which a browser may prefetch — and a GET must never write to the books. So
abo uses `templates/report.typ` if the company has put one there, and
otherwise its own, written fresh into `.abo/report.typ` each time. To change
the layout, copy that file into `templates/` and commit it.

```
ponytail: rendered statements are handed to you, not committed. A filed return
is arguably a document that belongs in the repository beside the bills. Add
`--file` writing under reports/<year>/ when a filing actually needs archiving.
```

### Nothing in the code knows where you are

Tax law is the one part of this that differs everywhere, so none of it is
compiled in. What the code knows is the *shape* of three schemes, because the
shape changes what a correct booking is:

| `[vat] method` | What it means | What a bill looks like |
|---|---|---|
| `none` | not registered | no VAT account may be posted to at all |
| `flat` | a flat percentage of gross turnover is remitted, no input tax deductible | one gross expense posting; an input-VAT posting is a **hard reject**, not a warning |
| `effective` | output tax collected less input tax paid | expense + input VAT + payable |

Everything else is configuration, which means changing it is not a rebuild:

- **`[invoice] vat_rate`** — the rate charged to customers. Statutory, therefore
  not in the code. `--vat-rate` overrides it per invoice, which is how an export,
  a reduced rate or an exempt supply is issued.
- **`[company] tax_notes`** — free text handed to the model with every bill:
  what is deductible here, what is reverse charged, how import tax arrives, what
  the scheme is called locally. This is where local rules live.
- **`[company] country`** — only the model reads it.
- **`[[report.vat]]`** — the tax return, one entry per box on the form. Each
  has a `code` and a `label`, and is one of three things: a `query` (any hledger
  query; its balance over the period is the figure, with `invert` for the
  credit side), `of` + `rate` (this box is a percentage of an earlier box), or
  `sum` of earlier codes where a leading `-` subtracts. Three forms cover
  every return shape seen so far; if a fourth is needed, it is a small
  evaluator in `statement.rs` and still not a rate in the code.

`abo vat` with nothing configured is an error that says what to write, rather
than a plausible wrong answer. The same rule covers other commodities: a return
is in the company currency, so each box is `hledger balance --value=then,<cur>`,
valuing anything else at the price on the day of the posting from the books'
own `P` directives, and a box that still holds another commodity — no price to
value it with — is refused, naming the directive to add.

Under `flat`, an invoice still charges the ordinary rate to the customer; the
flat percentage is what gets remitted, computed on gross turnover when the return
is filed, not per invoice. So `flat` books the whole invoice to income and posts
no output tax.

If somewhere needs a fourth shape, add a variant — but check first whether it is
really a shape and not just a rate under a different name.

### Config

```toml
[company]
name      = "Example Ltd"
address   = "1 Example Street\n10000 Example City"
country   = "Exampleland"
tax_id    = ""                 # blank if not registered
currency  = "EUR"
tax_notes = """..."""          # see Appendix A

[vat]
method  = "flat"               # "effective" | "flat" | "none"
rate    = 5.3                  # flat only: the percentage remitted on gross
account = "liabilities:vat"

[invoice]
vat_rate   = 8.1               # charged to customers
receivable = "assets:receivable"
income     = "income:sales"
terms_days = 30
decimals   = 2

[[payment]]                    # none, one or several, in the order invoices print them
id      = "postfinance"        # for commands: abo invoice --pay-to postfinance
label   = "PostFinance bank account"
details = """Account holder: Example Ltd
IBAN CH93 0076 2011 6238 5295 7
BIC POFICHBEXXX"""
account = "assets:bank:postfinance"   # declared; where the money arrives
```

```rust
#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "lowercase")]
pub enum Vat {
    None,
    Flat { rate: Decimal, account: String },
    Effective { input: String, output: String },
}
```

Exactly three places read the method: the AI system prompt, the validator, and
(later) the statement generator. Nothing else needs to know.

```
ponytail: one VAT method for all time. A report over a past period under a
previous method would need dated config. Filed returns are not regenerated, so
this only bites if the company switches methods and then re-runs an old period.
```

---

## 7. Interfaces

### CLI
One binary, clap subcommands. `--json` on every read command.

abo shells out to three programs and owns none of them, so `main` checks for
all three before dispatching anything. `git` and `hledger` are required and a
missing one is a startup error naming what it is for and where to get it;
`typst` is optional, needed only for PDFs, and a missing one is a warning
rather than a refusal — on stderr for the short commands, and through the log
beside the "listening on" line for `serve` and `demo`, since that is where
someone starting a server actually reads. Better than a puzzling failure three steps into a
workflow, and for `abo serve` it means the server declines to boot broken
instead of raising an error on the first request that needs a PDF.

### Web
axum + maud, server-rendered forms: balances and recent entries, bill upload and
proposal card, invoice form, reconciliation checkboxes, statements with a PDF
link, hand-typed entry.

A sidebar on the left, white cards with 12px radii on a warm stone ground,
and indigo for the primary button, links and the current page. The dark
palette is a dim blue-grey rather than near-black, with the sidebar only a
shade darker than the page. The figures along the top of the front page are
every asset account holding something, bar the VAT ones, and output less input
VAT when input tax is deductible. On a phone the sidebar becomes a bar of
icons across the top.

Notices come in five kinds — error, warning, information, change, success —
and each has its own colour and icon, one set for light and one for dark.
They never follow the palette: the colour says what kind of message it is, so
a palette change must not move an accent onto one of them.

IBM Plex Sans and Mono are named first and used where installed; otherwise
the system face. It is all one `STYLE` constant of custom properties, not
Bootstrap and not a font service: either from a CDN would mean the app stops
looking like itself offline and quietly tells someone else when the books are
opened, and vendoring means carrying and upgrading a framework for a grid and
some buttons.

Icons are inline SVG, drawn in `web.rs` on a 16-unit grid with one stroke
weight. That means no icon font, no extra request, no third-party licence to
track, and they take their colour from the text around them.

```
maud escapes interpolated text, including inside <style>, so a `>` child
combinator in the stylesheet silently becomes &gt; and the rule is dead. The
stylesheet is wrapped in PreEscaped for that reason. This cost an afternoon
once; it will not be spotted by reading the CSS.
``` `abo serve --port 8801` binds
127.0.0.1 and
runs until SIGINT or SIGTERM, finishing in-flight requests. TLS and any public
name belong to nginx in front of it; the Origin check works either way because
nginx forwards the public Host. `abo demo` is the same server over toy data.

Each request opens the books afresh — `abo.toml` and all — so an edit made
while the server runs, closing a period or changing a rate, applies to the
next request instead of after a restart. It is a small file read per request.

Every operation that succeeds redirects with `?done=<n>`, n being its line in
the log, and the page frame shows a success notice built from that line — never
from the query, so a link cannot put words on the page. It fades by CSS alone
after two seconds, held while pointed at or focused, and carries **Undo**. The
Log offers Undo on every line.

Undo and revert are two things. **Undo** takes an operation back: a commit's
change leaves the books as a new commit, and a pending bill is thrown away.
It is for a mistake just made. **Revert**, on a booking's own page, is the
bookkeeping correction of one that stands: a new entry with the opposite
amounts, dated today, tagged `reverses: <commit>`; the original stays.

Undo is not `git revert`. That merges with three lines of context, and since
entries are written in date order the next booking usually sits right beside
the one being undone, which it then reads as a conflict. Undo reverses exactly
the lines the commit changed (`git apply -R --unidiff-zero`), which git
refuses, changing nothing, if they are no longer as the commit left them — a
booking cleared since, say. `include` lines in main.journal are left, since
later entries share them, and a commit that started a year's file is
presented as lines added to an empty file, so undoing it removes those lines
rather than the file. The result gets the usual `hledger check`, and nothing
dated in a closed period may change; otherwise the patch is applied forward
again and the books are as they were. The message is git's own revert
message, "This reverts commit …", which is how the Log knows a commit was
undone, whether abo or a person at a terminal did it.

Forms that change the books carry a one-time token. A second send of one that
succeeded — a double click, a reload, the back button — is sent where the first
went rather than done again; a failed attempt spends nothing. Reconciling
redirects after each commit for the same reason, so reloading its page only
looks.

Logging is `tracing`, off for the short-lived commands and `abo=info` for these
two. `RUST_LOG` overrides it.

Separately, every operation someone asks for — an entry, a bill proposed,
corrected or approved, an invoice, clearing, recording a balance, a PDF —
appends one line to `.abo/log.jsonl`, from the CLI and the browser alike: when,
who (git's author name, until there are users), the input as given, and either
what came back, with its commit, or the error. Refusals are logged too; for
checking that what you did is what happened they are the most useful lines.
`/log` shows the last fifty, each opening to its input and, for a commit, the
diff it made. The log is working state like the rest of `.abo/` and is not
committed: who changed the books is already in git, and a shared record of
failed attempts would be noise in the books' history.

### Phone
A **PWA** over the same server-rendered pages: a manifest plus a responsive
stylesheet. The camera is a platform feature and needs no library:

```html
<input type="file" accept="image/*" capture="environment">
```

The bill workflow on a phone is photograph → read proposal → tap approve. That is
a form and two buttons. No JSON API is needed for this path at all.

Go native only when one of these is true: App Store distribution is required, the
iOS Share → Add to Home Screen install flow is unacceptable friction, or
background upload queueing beyond a service worker is needed. That is when the
JSON API earns its keep, and by then it is a second router over unchanged
functions.

---

## 8. Security

Two items are day-one, not deferred to exposure.

- **CSRF on every mutating POST.** Binding to 127.0.0.1 is not protection: any
  page open in the browser can POST to localhost. `SameSite=Strict` plus an
  Origin check. This shortcut is not available.
- **Never derive a filesystem path from an uploaded filename.** Build it from the
  sha256 and a sanitized vendor string. Path traversal through a multipart
  filename is how a file-upload feature becomes a shell.

### The model is an untrusted input source

Treat model output exactly like a form field submitted by a stranger. Then
"jailbreaking" is not a category: persuading the model to *want* something is
only an attack if the model *holds* something. Three rules keep it holding
nothing. Each is cheap now and each is the sort of thing that gets given away
casually later.

1. **The model gets no tools.** It receives a document and returns one JSON
   proposal. No filesystem, no shell, no hledger, no network. If a future
   feature wants the model to look things up, it gets a fixed set of
   parameterized queries — `recent_entries(vendor)` — never a command string.
   The moment a model-supplied string can select *what runs* rather than *what
   is proposed*, this section is void and the whole design needs re-reading.
2. **abo never invokes a shell.** `Command::new("hledger").args([...])` with an
   argument vector, never `sh -c`. No string concatenation into a command line.
   With no shell there is no shell injection, whatever the model writes.
3. **Every model-supplied string is validated before it touches disk.** The
   validator, not the prompt, is the boundary. It runs on entries from every
   source, so the CLI and the web form are covered by the same code.

The adversary is not hypothetical and does not require a hostile user: a
supplier emails a PDF, and the PDF contains text addressed to the model. What
that text can reach is (a) the strings in the proposal and (b) nothing else.
Amounts parse as `Decimal` or fail; accounts must already be declared;
document paths are derived from the sha256, never from the model. What remains
is the free-text fields, and journal syntax is what they can smuggle — a
newline opens a second transaction, and because hledger reads tags out of
comments, a `;` in a vendor name forges the `sha256:` tag that the duplicate
check depends on. Step 0 of the chain rejects control characters, `;` and
over-long values.

The prompt is not a security control. Nothing about the system may depend on
the model refusing to do something.

Deferred, with triggers:

- **Auth.** Not needed on localhost. When first exposed: TLS + HTTP basic auth at
  a reverse proxy, not a login page in the app. That plan expires when users
  exist — a phone client with basic auth is poor, so users mean session cookies
  and a login page. Build it then; but keep invariant §3.1 so authorization has a
  place to go.
- **Users and companies.** One repo per company still works. What does not fit in
  a books repository is the user table — credentials do not belong in the books.
  That is the first non-git store in the system; `users.toml` or a small SQLite
  outside the book repositories is sufficient for a long time.

---

## 9. Third-party obligations

- **hledger** — GPL-3.0-or-later, invoked as a separate process with command-line
  flags, output parsed from stdout. Not distributed with `abo`, and a separate
  process communicating this way is not a derivative work. **No obligations.**
  If it is ever bundled (a Docker image counts as distribution), GPL-3 §6 requires
  corresponding source: mirror the exact upstream tarball beside the image rather
  than linking to GitHub. §5 mere aggregation means `abo` itself is not affected
  either way. hledger is GPL, not AGPL — serving output over HTTP triggers nothing.
- **typst** — Apache-2.0, permissive, not copyleft. Shelled out and not
  distributed: no obligations. If bundled: include the licence text and NOTICE,
  keep copyright notices, mark modified files if modified. No trademark rights —
  do not name the product after it.
- **Bundled font** (EB Garamond / Source Serif 4 / Libre Baskerville, all OFL):
  keep the copyright notice with the font file, do not sell the font alone, and
  if modified, rename and drop any Reserved Font Name.

Not legal advice, but none of this is controversial.

---

## 10. Milestones

- **M0 — foundation, no AI.** `abo init` (six prompts → `abo.toml`, seeded
  `accounts.journal`, `.gitignore`, `git init`, first commit). `abo add` with the
  full validation chain. `abo bal` / `abo reg` passthrough. One test: a balanced
  entry commits; an unbalanced one is rejected; an input-VAT posting under
  `method = "flat"` is rejected. ~300 lines. **Done.**
- **M1** — `abo bill <file>`: AI proposal, terminal approve/refine. **Done.**
- **M2** — `abo serve`: balance, register, a booking form, and the bill upload
  and proposal card. **Done.** `abo demo` serves the same pages against a
  throwaway repository of toy data in the temp directory, rebuilt on every
  start, through the ordinary validated `add`.
- **M3** — invoices: typst template, numbering from the journal. **Done.**
  `templates/invoice.typ` is seeded into the books on first use and committed;
  it belongs to the company, not to abo. abo writes `.abo/invoice.json` and runs
  `typst compile`; the template does layout and never arithmetic, so rounding
  has one home. The next number is `max(invoice:) + 1` read from the journal —
  no counter file. The PDF is `git add`ed before the entry is committed, so both
  land in one commit or neither does.
- **M4** — reconciliation and statements. **Done.** `abo reconcile` / `abo
  clear` and a page of checkboxes; `abo pl`, `abo bs`, `abo vat` on screen,
  as `--json`, and as PDFs through a second typst template. Bank import moved
  to §11 — the point of reconciliation is to notice what is wrong, and typing
  a closing balance is enough to do that.
- **M5** — companies and users. **← next**
- **M6** — PWA manifest, or the JSON API if native has been chosen.

Reconciliation was not in the original list of workflows and is the one that
recurs most: bills arrive a few at a time, the statement arrives all at once and
has to tie out.

---

## 11. Wish list

Wanted, but not specified closely enough to be a milestone yet. New entries go
here first; they graduate to §10 once the shape is clear. Statements and their
PDFs started here and are now §6 and M4.

### Bank import

Reading a bank's CSV export straight into the journal, `hledger import` with a
rules file, matching statement lines against booked payables automatically.

Deliberately **not** next. Manual reconciliation covers the thing that actually
matters — noticing that the books and the bank disagree — and it does so with
one number typed in, no per-bank CSV dialect, no rules file to maintain, and no
new way for the books to acquire entries nobody looked at. Import is worth
building when the volume makes ticking boxes tedious, and by then the
reconciliation view already says exactly what it has to produce.

The AI is worth more here than in bill entry when it does arrive, because
matching a statement line to a booked payable has genuine ambiguity in it,
where reading a total off an invoice does not.

### Payroll, with configurable deductions

Run a payroll: gross to net for each employee, the journal entry that books it,
and a payslip PDF. Deductions are configuration — social insurance, pension,
unemployment, withheld income tax and anything else are set by statute and by
scheme, differ everywhere, and change yearly, so by §6 not one of them is
compiled in.

The shape follows M3 again: abo computes from a configured table of deductions,
writes `.abo/payslip.json`, and `templates/payslip.typ` lays it out.

Three things to get right, because they are where payroll goes wrong:

- **An employer contribution is not a deduction.** A deduction reduces net pay;
  an employer contribution does not touch it and is an additional expense with
  its own liability. A deduction table therefore needs a share field, not just a
  rate, and the entry has four kinds of posting: gross salary expense, employer
  contribution expense, net payable to the employee, and one liability per
  scheme for what is owed to it.
- **Bases and ceilings.** Rates apply to a base that is often not gross —
  capped at a ceiling, floored at a threshold, or reckoned on a different
  definition of pay. So a deduction is `{ name, base, rate, floor, ceiling,
  share, account }`, and the arithmetic is ordered because one deduction's base
  can be another's result.
- **Year to date.** Ceilings are annual, so a run needs what has been paid and
  withheld so far this year. That is already in the journal: `hledger` over
  prior runs for this employee supplies it, the same way invoice numbering reads
  `max(invoice:)` instead of keeping a counter. No payroll state file.

Open questions, to settle before this becomes a milestone:

- **Where employees live.** `payroll.toml` in the books repository is consistent
  with everything else and gets versioning and history for free. But it puts
  names and salaries in the repository — already half true of the journal, and
  fully true once payslips are committed. That is a §8 question, and it is the
  first thing in the design with a reason to be readable by some people and not
  others.
- **Progressive withholding.** A flat rate per deduction covers contributions
  but not banded income tax. Either a bracket table in the same config, or
  withholding is entered per employee and abo only books it.
- **Whether payslips are committed.** Same question as a rendered report, with
  more force: a payslip is a document the employee is owed, so probably yes,
  under `payslips/<year>/`, and then §8 grows a reason to care who can read the
  repository.

---

## 12. Explicitly not built

Each with the trigger for revisiting.

| Not built | Add when |
|---|---|
| Account / commodity editor UI | You are editing `accounts.journal` weekly. It is `$EDITOR` plus `hledger check accounts`. |
| Any abstraction over "AI provider" | There is a second provider. |
| Database | Never, for the books. Only for users (§8). |
| Client-side JavaScript | A workflow genuinely cannot be a form submission. |
| Bootstrap, Tailwind, or any CSS framework | The hand-written stylesheet stops fitting on one screen, or a component needs behaviour rather than style. |
| An icon font or an icon package | You need more than about twenty icons. |
| JSON API | A native phone client is chosen (§7). |
| Dated / versioned VAT config | The company switches methods and an old period must be reproduced. |
| Currency conversion | Never at booking time. For reporting, `hledger -X`. |
| A customer list | You retype the same address a third time. Past invoices are already in the journal. |
| QR or structured payment codes on invoices | A customer asks for one. It is a template change plus a QR crate, not an abo change. |
| Statutory rates in code | Never. They are `[invoice] vat_rate` and `--vat-rate`. |
| Auth in the application | Users exist (§8). |
| Templating engine, ORM, workspace split | Not foreseen. |
| A third typst template before it is needed | `templates/payslip.typ` arrives with payroll in §11, not before. |
| Committing rendered statements | A filing needs archiving. Today they are handed to you. |
| Closing entries at year end | A balance sheet needs to show a closed prior year. Today unclosed profit is a computed equity line. |

---

## Appendix A — an example `tax_notes`

Nothing below is in the code. It is what one company's `[company] tax_notes`
might contain, shown because it is the kind of thing the field is for: the
local rules a model needs in order to propose a correct booking, in prose,
in `abo.toml`, changeable without a rebuild.

```toml
tax_notes = """
Rates are 8.1% standard, 2.6% reduced (food, non-alcoholic drinks, books,
medicines), 3.8% accommodation.

This is a third country for EU VAT: there is no intra-community anything.
Goods crossing the border are imports and exports. Import tax is assessed by
customs and arrives on a SEPARATE customs document, not on the supplier
invoice. A foreign supplier's invoice showing no VAT therefore carries no
reclaimable input tax — the deductible amount is on different paper. This is
the most likely place to propose a wrong booking.

Services bought from abroad (hosting, SaaS, foreign consultants) are subject
to acquisition tax, self-assessed by the buyer. Frequent for a software
company. Flag them rather than guessing.

Foreign-currency bills convert at the published rate of the tax authority, so
the journal needs a price directive rather than a converted amount.

The return is filed by box number: 200-series turnover, 300-series tax,
400-series input tax. Quarterly under the effective method, semi-annual under
the flat scheme.
"""
```

An example return for the same company, in the same file:

```toml
[[report.vat]]
code   = "200"
label  = "Total agreed consideration"
query  = "acct:^income(:|$)"
invert = true

[[report.vat]]
code   = "302"
label  = "Tax at the standard rate"
query  = "acct:^liabilities:vat:output(:|$)"
invert = true

[[report.vat]]
code  = "400"
label = "Input tax on materials and services"
query = "acct:^assets:vat:input(:|$)"

[[report.vat]]
code  = "500"
label = "Amount payable"
sum   = ["302", "-400"]
```

Under a flat scheme the tax owed is a percentage of gross turnover rather than
a collected figure, which is the `of` form:

```toml
[[report.vat]]
code  = "322"
label = "Tax at the flat rate"
of    = "200"
rate  = 5.3
```
