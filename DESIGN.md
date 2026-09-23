# abo — accountant back office

Bookkeeping for a small Swiss company. A git repository holds the books; a CLI
operates on it; a web server wraps the same functions in a browser interface; an
AI proposes bookings from scanned documents which a human approves.

This document exists so that work can resume on a different machine with a
different agent. It records what was decided and **why**, so the decisions are
not re-litigated. Where something was deliberately left out, the trigger for
adding it is stated.

Status: M0 built, plus `abo serve` / `abo demo` (the web shell of M2, without
the AI). Next step is M1.

---

## 1. Decisions locked

| Decision | Choice | Why |
|---|---|---|
| Language | Rust | Stated requirement. |
| Storage | The git repository itself. No database. | Audit trail, versioning, undo, backup and sync become `git log`, `git revert`, `git push`. Approve = commit. |
| Journal format | hledger | Follows from the reports decision below. |
| Reports | Minimal Rust parser for writing and validating; **hledger computes everything**. | Multi-commodity, elided amounts, price directives and period expressions are where a hand-rolled money parser goes quietly wrong. hledger is already installed (GPL-3+, shelled out, no licence obligation — see §9). |
| Invoice PDF | typst template, `typst compile` shelled out | One page, fixed grid. Layout lives in a `.typ` template, not in Rust. Compiler is Apache-2.0, offline, no account. |
| Git access | shell out to `git` | Smaller than `git2`/libgit2 by a wide margin. |
| Web UI | axum + maud, server-rendered | No SPA, no npm, no build step, no WASM. HTML escaping comes from maud. |
| Deployment | localhost first, reachable later | See §8. Auth goes at a reverse proxy, not in the app — until users exist. |
| VAT | configured per company, `effective` / `saldo` / `none` | Not a preference. It changes what a *correct booking* is. See §6. |
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
  bills/2026/2026-09-21-acme-inv1234.pdf
  invoices/2026/2026-0017.pdf
```

Documents are linked to entries by hledger tags, never by a join table:

```
2026-09-21 Acme Hosting
    ; bill: bills/2026/2026-09-21-acme-inv1234.pdf
    ; sha256: 9f2a...
    expenses:hosting        100.00 CHF
    assets:vat:input          8.10 CHF
    liabilities:payable    -108.10 CHF
```

- `sha256:` is the duplicate check. Before booking a document, `hledger print tag:sha256=<hash>` must return nothing.
- `invoice:` on issued invoices carries the invoice number. The next number is
  `max(invoice:) + 1` read from the journal — no counter file, no state to corrupt.

Account naming: `assets:bank`, `expenses:telecom`. If a Treuhänder requires Käfer
/ Kontenrahmen KMU numbers, `assets:1020:bank` also sorts correctly.

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

Seven files. No `lib.rs`/`bin/` split, no workspace, no crate-per-module.

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
6. On approval: move the document to its final path, append the entry, commit.

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
  "currency": "CHF",
  "postings": [{"account": "expenses:hosting", "amount": "100.00"}],
  "invoice_ref": "INV-1234",
  "confidence": "high",
  "questions": []
}
```

`questions` is how the model asks rather than guesses — a non-empty list blocks
auto-approval even under `--yes`.

---

## 6. Validation and VAT

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
**Never `f64`.** Rates may be `Decimal`.

### Config

```toml
[company]
name     = "Example GmbH"
uid      = "CHE-123.456.789 MWST"
currency = "CHF"

[vat]
method  = "saldo"              # "effective" | "saldo" | "none"
rate    = 5.3                  # saldo only: the ESTV industry rate
account = "liabilities:vat"
```

```rust
#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "lowercase")]
pub enum Vat {
    None,
    Saldo { rate: Decimal, account: String },
    Effective { input: String, output: String },
}
```

Exactly three places read it: the AI system prompt, the validator, and the
statement generator. Nothing else needs to know.

### Swiss VAT specifics

Statutory rates are federal and identical for every company, so they are
**constants in code**, not per-company config: standard **8.1%**, reduced **2.6%**
(food, non-alcoholic drinks, books, medicines), special **3.8%** (accommodation).
Unchanged for 2026; last changed 2024-01-01. An optional `[vat.rates]` override
table covers the gap between a future rate change and a rebuild. The rates are
not needed to *book* (the supplier states the VAT amount) — only to *check* that
the stated amount equals net × rate.

Four things that differ from EU VAT and must be in the system prompt:

- **Saldosteuersatz.** Under the net tax rate method you pay a flat industry rate
  on gross turnover and deduct **no input VAT at all**. A bill is a single gross
  expense posting. Filing is semi-annual. An input-VAT posting under
  `method = "saldo"` is a hard reject in the validator, not a warning.
- **No intra-community anything.** Switzerland is a third country for EU VAT.
  Goods crossing the border are imports and exports. Import VAT is assessed by
  customs (BAZG) and arrives on a **separate customs document**, not on the
  supplier invoice. A German supplier's invoice showing zero VAT carries no
  reclaimable input tax — the deductible amount is on different paper. This is
  the most likely place for the model to propose a wrong booking.
- **Bezugsteuer** (acquisition tax) on services bought from abroad — AWS, GitHub,
  foreign consultants. A genuine reverse charge, self-assessed, and frequent for
  a software company. Applies under the effective method.
- **The return** is ESTV Ziffern (200-series turnover, 300-series tax, 400-series
  input tax) filed via ePortal, not an EU-country form. Statement config maps
  account globs to box numbers. Period length: quarterly for effective,
  semi-annual for saldo.

Foreign-currency bills convert at the ESTV published rate, so `abo.toml` needs a
rate source and the journal needs price directives.

```
ponytail: one VAT method for all time. A report over a past period under a
previous method would need dated config. Filed returns are not regenerated, so
this only bites if the company switches methods and then re-runs an old period.
```

---

## 7. Interfaces

### CLI
One binary, clap subcommands. `--json` on every read command.

### Web
axum + maud, server-rendered forms. `abo serve --port 8801` binds 127.0.0.1 and
runs until SIGINT or SIGTERM, finishing in-flight requests. TLS and any public
name belong to nginx in front of it; the Origin check works either way because
nginx forwards the public Host. `abo demo` is the same server over toy data.

Logging is `tracing`, off for the short-lived commands and `abo=info` for these
two. `RUST_LOG` overrides it.

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
  `method = "saldo"` is rejected. ~300 lines. **Done.**
- **M1** — `abo bill <file>` **← next**: AI proposal, terminal approve/refine.
- **M2** — `abo serve`: balance, register and a booking form. **Done.** The
  upload form and proposal card land with M1. `abo demo` serves the same pages
  against a throwaway repository of toy data in the temp directory, rebuilt on
  every start, through the ordinary validated `add`.
- **M3** — invoices: typst template, numbering from the journal.
- **M4** — bank import and reconciliation, then statements (VAT return, P&L,
  balance sheet).
- **M5** — companies and users.
- **M6** — PWA manifest, or the JSON API if native has been chosen.

**Bank import was not in the original list and should not be dropped.** Bills
arrive a few at a time; the statement arrives all at once and has to tie out.
Matching statement lines against booked payables is the recurring monthly work,
and it is where the AI is worth more than in bill entry, because there is genuine
ambiguity to resolve. `hledger import` already handles CSV → journal via rules
files.

---

## 11. Explicitly not built

Each with the trigger for revisiting.

| Not built | Add when |
|---|---|
| Account / commodity editor UI | You are editing `accounts.journal` weekly. It is `$EDITOR` plus `hledger check accounts`. |
| Any abstraction over "AI provider" | There is a second provider. |
| Database | Never, for the books. Only for users (§8). |
| Client-side JavaScript | A workflow genuinely cannot be a form submission. |
| JSON API | A native phone client is chosen (§7). |
| Dated / versioned VAT config | The company switches methods and an old period must be reproduced. |
| Auth in the application | Users exist (§8). |
| Templating engine, ORM, workspace split | Not foreseen. |
