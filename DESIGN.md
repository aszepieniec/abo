# abo — accountant back office

Bookkeeping for a small company. A git repository holds the books; a CLI
operates on it; a web server wraps the same functions in a browser interface.
Every change to the books is one `abo` command, and the log records it as that
command. `abo` asks no model anything; a model belongs in a larger application
that helps a person arrive at the command (§5).

This document exists so that work can resume on a different machine with a
different agent. It records what was decided and **why**, so the decisions are
not re-litigated. Where something was deliberately left out, the trigger for
adding it is stated.

Status: M0-M4 built — `abo init`, `post`, `file`, `bill`, `invoice`,
`reconcile`, `record`, `unrecord`, `revert`, `undo`, `company`, `payment-options`, `pl`, `bs`,
`vat`, `balance`, `register`, `log`; `abo-web serve` and `demo`. M5: `abo-web serve` serves
one company; users come from the gateway, in front of it.
M6 built: each company installs as an app; not yet tried on a real phone
(WORKFLOWS.md G1). Partners and agreements (MODEL.md §2.9): `partner`,
`agreement`, and their pages under Partners.

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
| Deployment | localhost; behind a gateway when reachable | abo is not responsible for authentication: no login, no users, no TLS. The gateway, in front, is all three. See §8. One exception: a download link, an address that is its own key, for a script to fetch the books whole. The gateway has no credential to give a script, and the key opens one read-only route and nothing else. |
| Tax | three scheme *shapes* in code (`none` / `flat` / `effective`); every rate, rule and name in config | The shape changes what a *correct booking* is, so the validator must know it. Rates and local rules differ everywhere and are data. See §6. |
| Commodities | multi-commodity from the start | hledger already is. Cash, bank and wallet balances are all just asset accounts. |
| Phone app | PWA over the existing server-rendered pages | See §7. |
| Questions | every command that asks takes `--ni`, and then asks nothing: each question takes what it offers, as a flag left out does, and a yes or no is no | A script, a model's tool call or the gateway must never wait on stdin. Under `--ni` a question reached all the same is an error, not a wait (`tests/regressions.rs` `ni_asks_nothing`). A new command that asks adds its case there. |
| `abo init` | opens the books in the directory it is run in, or `--dir`, which is there and empty | As `git init`. The web form does the same on an empty directory, so a project behind a gateway is one company. |

### Crates

`clap serde toml serde_json tokio rust_decimal jiff sha2 anyhow
unicode-general-category`; and `axum maud` in abo-web.

The last answers what kind of character a character is, from the Unicode
tables: text holding one that prints as nothing or turns the text around it
is refused by its category, not from a list kept by hand, which the
verification runs kept finding short.

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
  templates/credit-note.typ
  bills/2026/2026-09-21-acme-a1b2c3d4.pdf
  invoices/2026/2026-0017.pdf
  invoices/2026/2026-0017.json   # what the invoice says, and its PDF's hash
  bills/index             # every document: its hash, path and bill
  partners.toml           # whom invoices go to, bills come from, salaries are paid to
  contracts/acme-hosting.toml   # an agreement: parties, status, versions
  contracts/acme-hosting/…      # its signed contract and amendments
  contracts/index         # every contract document: its hash, path and version
  .abo/                   # gitignored: typst's scratch files, the lock
```

What the files say, and how, is MODEL.md's: §2 the state, §7 its one text.
An entry carries its id and its kind as tags:

```
2026-09-21 Acme Hosting AG, hosting September
    ; id: 412
    ; bill:
    Expenses:Hosting                              100.00 EUR
    Assets:VAT:Input                               10.00 EUR
    Liabilities:Payable                          -110.00 EUR
```

- A bill's document names its bill, in `bills/index`, by the bill's id; its
  hash is also the duplicate check.
- `invoice:` carries an invoice's number, which is its place in its year:
  counted, not stored. `credits:` on a credit note names the invoice it
  cancels, by id; it is numbered in the same series, and its entry is the
  invoice's negated.
- `pays:` on a payment names the one debt it pays, by id. Whether a debt is
  settled is what the amounts say, never a tag.
- `partner:` names who a bill or an invoice is with. An invoice issued under
  an agreement also carries `agreement:` (MODEL.md §2.9).

A partner is a name, with an address and a tax number where something
prints them; an employee is a partner. An agreement is a contract with one
or more partners, held as versions only added, each with its signed copy,
and active until terminated or subsumed by another on a day. A version may
carry an invoice template: the invoice form's fields an invoice issued
under it starts from. An agreement holds no schedule: nothing falls due
under it, and abo makes nothing of its own accord.

Account naming: CamelCase segments, acronyms in capitals: `Assets:Bank`,
`Expenses:Telecom`, `Liabilities:VAT:Output`. If your accountant wants a
numbered chart of accounts, `Assets:1020:Bank` also sorts correctly.

---

## 3. Source layout

```
src/main.rs       abo: the command line, as the library runs it
src/lib.rs        the library under both binaries: the programs abo runs, the CLI
src/cli/          the command line: clap's args, each command run, the terminal's questions
src/model/        the books as MODEL.md defines them: state, check, operations, text,
                  what a person may type (answer.rs), refusals said of the operation
src/store.rs      the books on disk: read through the model, change by an Op, commit, log
src/command.rs    an Op as the command that makes it, and as its commit's subject
src/input.rs      what a person types, read: dates, amounts, postings, entries by number
src/views.rs      what the books show, as data: invoice rows, reconciling, money
src/questions.rs  init's and a payment option's questions, for the terminal and the form
src/repo.rs       finding the books, git, the lock, slugs
src/statement.rs  P&L and balance sheet, from hledger
src/invoice.rs    issuing an invoice or credit note, and its PDF's data
src/pdf.rs        typst: a template and its data made a PDF
src/report.rs     hledger passthrough (-O json / -O csv)
abo-web/src/main.rs   abo-web: serve and demo, and the server's log
abo-web/src/demo.rs   the demo's toy books
abo-web/src/web/      axum + maud: a module per page's area, the forms, the opening
                      form, Chat and Scan (ai.rs), and style.css
```

A workspace of three crates, each a binary: `abo`, the command line, also
the library the other two build on; `abo-web`, the web interface; the model's
program (§5). abo-web needs abo's library and nothing else; the model's program needs
neither, and runs `abo` as a person does. No crate-per-module.

The web tests stay in abo's crate (`src/web_tests.rs`, `tests/web.rs`): they
hold the pages to the model and to the command line with abo's test helpers,
and run the `abo-web` binary, built for them beside `abo`
(`tests/common/mod.rs`).

The tests are run with `cargo test --no-fail-fast`: without the flag cargo
stops at the first test binary that fails and never runs `tests/`. Two in
`src/model/tests.rs`, `every_rule_is_checked` and `every_guarantee_is_tested`,
hold MODEL.md and the code to each other: a rule or a guarantee written
ahead of the code fails them until it is built.

### Three invariants that keep future work cheap

These are free now and expensive to retrofit. Do not violate them.

1. **Everything acts on the `Store` it is handed.** A `Store` is one
   company's books, opened (`Store::open`): their directory and what was read
   from it. Every change is `store.change(op)`; nothing reads a global
   company. Who is acting is the gateway's to say (§8), and is carried per request
   (`repo::AUTHOR`), not by the `Store`.
2. **No rendering below the dispatch layer.** No HTML, no terminal formatting, no
   `println!` inside workflow functions. They return serde structs.
3. **`--json` on the CLI is the API schema.** The struct a phone client would
   receive is the struct the CLI already prints, so it is exercised from day one.
   A JSON API later is a second axum router over unchanged functions.

The web server calls these functions **in-process**, through abo's library. It
does not shell out to the CLI and it does not talk to itself over HTTP. Chat and
Scan are the exception: they are the program's, which abo-web runs (§5).

Questions asked in both places are written once. `src/questions.rs` describes
opening the books (and a payment option's questions) as data: text, lines, a
choice with the questions that follow each option, questions shown only for
some answers, and a repeated group such as payment options, each question
with what it offers when left blank and how its answer is checked. The
terminal walks the description asking one question at a time; the browser
draws it as one form, and reads a posted form by walking it again. Both hand
the same answers to one function that turns them into books. A test gives the
same answers to `abo init` and to `/new` and requires identical books.

---

## 4. The bill workflow (the core)

Two commands, each a commit and a line in the log.

1. `abo file <scan> --date <date> --issuer <name>` copies the document to
   `bills/<year>/<date>-<issuer-slug>-<sha8>.<ext>` and commits it. The content
   type comes from the bytes. A document whose hash is filed already is
   refused.
2. `abo bill --desc <text> --document <bills/...> <postings>` books a bill
   against a filed document, dated as the document is, and the document names
   the bill (MODEL.md §2.5). A document whose bytes have changed since it was
   filed is a problem `abo check` names, and the books are not changed until
   it is mended.

The issuer slug keeps only `[a-z0-9]` and collapses the rest to `-`, so an
issuer of `../../.ssh/authorized_keys` cannot escape the directory. There is a
test for exactly that. The supplier's own invoice number goes in the
description; `invoice:` is the numbering of invoices we issue.

Document filenames are derived from the date, the issuer slug and the sha256 —
**never** from a client-supplied filename (§8). The browser's **Book a bill**
form asks for all of it at once and runs both commands, the entry validated
before the document is filed, so a bill that cannot be booked is not filed.

A filed document that is not booked stays in the books; undoing its `abo file`
line removes it. Once it is booked, that undo is refused: the booking names it.

---

## 5. Where a model fits

Not in `abo`: in the model's program, a second crate in this repository and a binary of
its own, which runs `abo` as a person at a terminal would.

- `scan <file>` sends a bill's document (PDF, JPEG, PNG or WebP) to
  the model, which transcribes it and proposes the `abo file` and `abo bill`
  that book it.
- `chat` is a conversation about the books.

A photo over the API's 5 MB is sent smaller, as a JPEG no side of which is
longer than the 2576 pixels the current models read (`shrink`, with its
test); what `abo file` files is the photo as taken.

Both are one conversation, the same loop. The model has two tools. `document`
reads a document filed in the books, by its path: one under `bills/` or
`contracts/` and nowhere else, links followed before the check. `abo` takes a
command's arguments. A command that only reads the books
(`reads` in the model's program, with its test) runs at once and its output
goes back to the model. Any other command is printed, and the person runs it
with `y` or answers in words, which go back to the model in its place. Each
command runs with `--ni`, so `abo` sees a typed command: it validates it (§6)
and logs it, and nothing in the books depends on what a model said.

The key is `ANTHROPIC_API_KEY`; the model is `claude-opus-5-5`, with server-side
fallback on a refusal. `ANTHROPIC_BASE_URL` and a model set in the environment point it
elsewhere, such as a local Ollama (`http://127.0.0.1:11434`, which speaks the
same API) for `chat`; a small local model reads documents badly, so `scan`
wants Claude. Requests are plain HTTPS (`ureq`): there is no official
Rust SDK. At a terminal the conversation is not kept: it ends with the process.

### In the browser

abo-web's **Chat** and **Scan** tabs are there when the model's program is installed, beside
abo-web or on the `PATH`, and the person may change the books: every message
may lead to a change. Without it they are not shown, and their pages are not
found (`tests/web.rs`). abo-web links
nothing of the program's: it runs the executable, a step at a time.

- `scan <file> --to <conversation>` begins one with a document;
  `say <conversation> <text>` adds a message, or, to a command
  waiting, says it instead of running it; `run <conversation>` runs
  the command waiting. Each carries the conversation on until it waits on
  the bookkeeper again: the model has had its say, or a command that
  changes the books waits for a yes.
- `show <conversation>` gives it as JSON: its title, and what each
  said and each command with what came of it, which the page draws under
  the title. The API's own format
  stays the program's.
- The model's answers are rendered as markdown (`pulldown-cmark`): tables,
  lists, emphasis. What the model writes is not trusted, since a scanned
  document can tell it what to write. So HTML in an answer is shown as
  text. An image is dropped and its description kept, since its address
  could carry the books to another site as the page loads. A link is one
  only within abo's pages (`/bills/...`); any other, `//host`, `/\host`
  and `javascript:` among them, is its text alone (`markdown` in
  `abo-web/src/web/ai.rs`, with its test). On the page the model is told
  that markdown is rendered; at a terminal it is asked for plain ASCII.
- `list <directory>` gives the conversations kept there, newest
  first, each with its title: what it is about and what came of it, in a
  few words, made anew by a small model after each step and kept in the
  conversation. The model is the model set for titles, else the one set for the rest,
  else `claude-haiku-5-5`. A local model that thinks at length before it
  answers (qwen3.5 on Ollama, say) never gets to the title, so give it
  one that does not; without a title, a conversation is listed by its
  first words.
- A conversation is kept in the books' `.abo/ai/`, outside git, as
  `<id>.json`, and a scanned document beside it, from which `abo file`
  files it.
- A command runs under the name of the person the gateway names, as a
  change made on any page: abo-web passes it as `GIT_AUTHOR_NAME` and
  `GIT_AUTHOR_EMAIL`.
- One step at a time per conversation: a second Run, or a message sent
  while the model's program is still working, is refused, so a command never runs twice.
- A step waits for the model, which can take minutes. The page is a plain
  form, as every other is, so it waits too.

An earlier version called the Messages API from `abo bill` itself (commit
fb6ac2c). It was taken out so that `abo` is model-free, and `abo` stays so.

---

## 6. Validation, money and tax

### The validation chain

Every change is an operation of the model (MODEL.md §5), whichever page or
command makes it: `model::apply` builds the next state and keeps it only if
every invariant (§4) and every condition on the change holds of it; refused,
nothing is written, and the refusal names the rules. The store then writes
what changed and commits it. hledger reads the files after, and the property
tests check that it reads them as the model does (§3, G-H1 to G-H4).

Money is `rust_decimal::Decimal`, at most 8 places, so that sums are exact
(MODEL.md I-K0, I-K3). **Never `f64`.** Rates may be `Decimal`.

### Commodities

The books are multi-commodity from the start, because hledger is. A posting
carries an amount and a commodity, the validator balances each commodity
separately, and nothing anywhere assumes there is only one. Bank balances, notes
in a drawer and a wallet are all just asset accounts:

```
Assets:Bank      Assets:Cash      Assets:Wallet:BTC
```

`abo init` seeds `Assets:Bank` and `Assets:Cash`; anything else is an `account`
line in `accounts.journal`, and a second commodity is a `commodity` line. An
invoice can be denominated in any of them (`abo invoice --currency BTC`), and
`[invoice] decimals` — or `--decimals` — says how many places that commodity is
stated in: two for most money, eight for a wallet. Rounding happens in exactly
one place, `invoice::round`, half away from zero.

A payment option's fields are free text for the same reason: a wallet
address, a bank's address, "at our office, weekdays 9-17". abo never parses
them, it only prints them. The one exception is the IBAN, whose check digits
are checked, because a mistyped IBAN on an invoice sends a customer's money
nowhere. The option also has an id for commands, a name for invoices and the
account the money arrives in; `abo payment-options` adds, edits and removes
options.

Amounts are never converted between commodities. A bill in one currency against
books in another is booked as stated and the model is told to say so. Valuing a
report in a single commodity is `hledger -X`, at the point of reporting, where
the rate is a reporting choice rather than a fact about the transaction.

### Reconciliation

Agreeing the books with what a bank, a cash box or a wallet actually holds is
manual. What is agreed is recorded as a statement (MODEL.md §2.6): an entry
that moves nothing and names the statement's balance and the postings it
clears. It is the posting that is cleared, not the transaction: a transfer from
the bank to the cash box is seen by two statements, often on different days,
and ticking it off against one must leave it outstanding against the other.

```
abo reconcile --account Assets:Bank --date 2026-04-30 --balance 24390.00
abo record --account Assets:Bank --date 2026-04-30 --balance 24390.00 412.2 431.1 ...
abo unrecord --account Assets:Bank
```

`reconcile` changes nothing. It prints what is in the books on that date, what
the recorded statements clear, what the statement says, the difference between
those two, and every posting not yet cleared. Ticking postings until the
difference is zero *is* the reconciliation; `record` writes what was ticked,
or `--all` of it. The browser shows the same thing with checkboxes, and its
two buttons are `record` and `unrecord`.

Three details that are easy to get wrong:

- **Clearing is written in the statement, not on the entry.** hledger's own
  mark, a `*` on the posting, is not used: writing it would change an old
  entry's text each time a later statement cleared one of its postings, and an
  entry's text is fixed when it is added (MODEL.md §7.3). A posting is named
  `ENTRY.N`, its entry's id and its place in it, so a name stays right however
  the journal's lines move.
- **The balance assertion is written only for a whole statement.** hledger's
  balance assertions are computed over every posting regardless of status, so
  there is no way to assert a cleared-only balance. When the statement and the
  earlier ones clear every posting up to its date, the two numbers are the
  same and the assertion is exactly true, so it is written and hledger
  re-checks it on every read. Otherwise the statement figure is *not* the
  account balance, and only abo checks it (I-S3).
- **A statement fixes the past.** Once one is recorded, nothing is booked to
  its account in its commodity on or before its date (N7), and only the latest
  can be taken out, since later ones count what it cleared.

Entries are inserted in date order rather than appended, which is what makes a
late-arriving bill ordinary rather than a failure of `hledger check
ordereddates`. That applies to every write, not just this one.

### Reports

`abo pl` and `abo bs`, each with `--json` and `--pdf <file>`, and each a
page under `/report/`. Everything is computed by `hledger balance` over a query,
read back through the one JSON shape `statement.rs` parses — mantissa
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
  "Net income" line (income less expenses, not yet closed), and the bottom line —
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
- **`[company] country`** — printed on invoices.

abo makes no tax return (§12).

Under `flat`, an invoice still charges the ordinary rate to the customer; the
flat percentage is what gets remitted, computed on gross turnover when the return
is filed, not per invoice, and not by abo. So `flat` books the whole invoice to
income and posts no output tax.

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

[vat]
method  = "flat"               # "effective" | "flat" | "none"
rate    = 5.3                  # flat only: the percentage remitted on gross
account = "Liabilities:VAT"

[invoice]
vat_rate   = 8.1               # charged to customers
receivable = "Assets:Receivable"
income     = "Income:Sales"
terms_days = 30
decimals   = 2

[[payment]]                    # none, one or several, in the order invoices print them
id      = "postfinance"        # for commands: abo invoice --pay-to postfinance
kind    = "bank"               # "bank" | "cash" | "crypto"
label   = "PostFinance bank account"
holder  = "Example Ltd"        # bank: holder, iban, bic, bank
iban    = "CH93 0076 2011 6238 5295 7"
bic     = "POFICHBEXXX"
bank    = "PostFinance AG\nMingerstrasse 20, 3030 Bern"
account = "Assets:Bank:PostFinance"   # declared; where the money arrives

[[payment]]
id      = "wallet"
kind    = "crypto"             # crypto: network, address
network = "Bitcoin"
address = "bc1q..."
details = "Please send the exact amount"  # any kind: further lines
account = "Assets:Crypto"
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

Exactly one place reads the method: the validator. The model reads it
with `abo config`, as a person would.

```
ponytail: one VAT method for all time. A report over a past period under a
previous method would need dated config. Filed returns are not regenerated, so
this only bites if the company switches methods and then re-runs an old period.
```

---

## 7. Interfaces

### CLI
One binary, clap subcommands. `--json` on every read command.

A command that changes the books takes everything it needs as arguments. One
that asks — `abo init` without flags, `abo payment-options add` without flags
— asks only to arrive at those arguments: what it logs is the command the
answers amount to, and running that command instead asks nothing. `abo init`
takes a flag per question, a payment option as `--payment` with a TOML inline
table of its fields, and with any flag given asks nothing, taking for what is
not given what the question offers.

abo shells out to three programs and owns none of them, so `main` checks for
all three before dispatching anything. `git` and `hledger` are required and a
missing one is a startup error naming what it is for and where to get it;
`typst` is optional, needed only for PDFs, and a missing one is a warning
rather than a refusal — on stderr for the short commands, and through the log
beside the "listening on" line for `abo-web serve` and `demo`, since that is where
someone starting a server actually reads. Better than a puzzling failure three steps into a
workflow, and for `abo-web serve` it means the server declines to boot broken
instead of raising an error on the first request that needs a PDF.

### Web
axum + maud, server-rendered forms: balances and recent entries, the bill
form, invoice form, reconciliation checkboxes, statements with a PDF
link, hand-typed entry.

A sidebar on the left, white cards with 12px radii on a warm stone ground,
and indigo for the primary button, links and the current page. The dark
palette is a dim blue-grey rather than near-black, with the sidebar only a
shade darker than the page. The figures along the top of the front page are
every asset account holding something, bar the VAT ones, and output less input
VAT when input tax is deductible. On a phone the sidebar becomes a bar of
icons across the top.

Each item in the sidebar is one word, and the word need not be the model's:
`/declarations` is Accounts, `/post` is Entry. Backups, last, is admin's
alone (§8): the books whole as a file, a newer copy brought in, and download
links. It is the server's keeping of the books, not the company's, so it is
not on the company page. Partners and agreements
(MODEL.md §2.9) will be one item, Partners. An agreement is reached from the
pages of its parties, and its own pages keep Partners as the current item.

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

Icons are inline SVG, drawn in `web/mod.rs` on a 16-unit grid with one stroke
weight. That means no icon font, no extra request, no third-party licence to
track, and they take their colour from the text around them.

```
maud escapes interpolated text, including inside <style>, so a `>` child
combinator in the stylesheet silently becomes &gt; and the rule is dead. The
stylesheet is wrapped in PreEscaped for that reason. This cost an afternoon
once; it will not be spotted by reading the CSS.
``` `abo-web serve --port 8801` binds
127.0.0.1 and
runs until SIGINT or SIGTERM, finishing in-flight requests. TLS and any public
name belong to nginx in front of it; the Origin check works either way because
nginx forwards the public Host. `abo-web demo` is the same server over toy data.

Each request opens the books afresh — `abo.toml` and all — so an edit made
while the server runs, closing a period or changing a rate, applies to the
next request instead of after a restart. It is a small file read per request.

Every operation that succeeds redirects with `?done=<n>`, n being its line in
the log, and the page frame shows a success notice built from that line — never
from the query, so a link cannot put words on the page. It fades by CSS alone
after two seconds, held while pointed at or focused, and carries **Undo**. The
Log offers Undo on every line.

Undo and revert are two things. **Undo** takes a line of the log back: its
commit's change leaves the books as a new commit. It is for a mistake just
made. The first line, `abo init`, cannot be undone, nor can the filing of a
document that is booked. **Revert**, on a booking's own page, is the
bookkeeping correction of one that stands: a new entry with the opposite
amounts, dated today unless `--date` says otherwise, tagged `reverses:
<commit>`; the original stays.

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

Separately, every change to the books writes one line of the log, from the
CLI and the browser alike, and nothing else does: not a read, not a
refusal, not a PDF of a report. A line is the `abo` command that made the
change, as an argument vector, with every default it used written out — the
date that was today, the VAT rate from `abo.toml`, the payment options an
invoice printed — so that the same command means the same thing on any day.
The first line is always `abo init`. An action in the browser is logged as the
command it amounts to; the bill form is two, `abo file` and `abo bill`.

A line is kept in the message of the commit the change makes, after its
subject: the command as it would be typed, to copy into a terminal.

    Book Two words

    abo post --date=2026-11-05 '--desc=Two words' -- 'Expenses:General=3 EUR' 'Assets:Bank=-3 EUR'

A commit of two operations, as the bill form makes, has two commands, each
after a comment that says what it did, so that the two can be pasted
together. The subject and the comments are put on one line first, so that no
text typed into them makes a line of its own. The stamp is git's: the author
name and email, and the second the operation began, as both author and
committer date. An invoice PDF carries the stamp as its creation date. So the
log goes wherever the history goes — a clone, a bundle, a mirror — and undo
there takes back the last change as it would here. Books from before kept
their lines in `.abo/log.jsonl`; it is read first, as all of them are older.
The log is a record of what was done, not a source of the books: they
are read from their files alone, and the git history is what restores them
(MODEL.md §5.4).

`/log` and `abo log` show the lines, each with the command as it would be
typed and, on `/log`, the diff its commit made.

### Phone
A **PWA** over the same server-rendered pages: a manifest plus a responsive
stylesheet. The camera is a platform feature and needs no library:

```html
<input type="file" accept="image/*,application/pdf">
```

The bill workflow on a phone is photograph → date, issuer and postings →
submit. That is one form. No JSON API is needed for this path at all.

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
  date, a sanitized issuer string and the sha256. Path traversal through a multipart
  filename is how a file-upload feature becomes a shell.

### Every string is an untrusted input

A description, an issuer, a tag value: whatever typed or suggested it, a
string reaches the journal only through the validator, which runs on entries
from every source. Two rules keep that boundary where it is.

1. **abo never invokes a shell.** `Command::new("hledger").args([...])` with an
   argument vector, never `sh -c`. No string concatenation into a command line.
   With no shell there is no shell injection, whatever a field holds.
2. **Every string is validated before it touches disk.** Amounts parse as
   `Decimal` or fail; accounts must already be declared; document paths are
   derived from the date, the issuer slug and the sha256. What remains is the
   free-text fields, and journal syntax is what they can smuggle — a newline
   opens a second transaction, and because hledger reads tags out of comments,
   a `;` in a description forges a tag. What each field may hold is the
   table of what hledger reads back as written, asked of hledger itself
   (MODEL.md §2.3), with every hidden character refused.

This matters most for the application a model lives in (§5). A supplier's PDF
can contain text addressed to the model, and what that text can reach is the
command the model suggests, which `abo` treats as it treats a typed one.

### Authentication is not abo's

abo has no login, no users and no TLS, and never will: whoever reaches its
port may do anything to the books it serves. It listens on 127.0.0.1 only.
Run alone, that is one person on their own computer. Reachable by others,
it runs behind a gateway, which is the login (accounts, sessions, second
factors), the roles on a project (view, write, admin), and TLS. The gateway runs
each project's abo in a sandbox on a network of its own, so the gateway is the
only one that reaches the port.

What abo takes from the gateway is who is asking: `X-Gateway-User`, `X-Gateway-Name`
and `X-Gateway-Email`, the author of the commits a request makes. It trusts
them because only the gateway can send them; the gateway replaces any a browser sends.
A role abo does not check: the gateway refuses a viewer's POST before abo sees it.
It only shows it: given `X-Gateway-Role: view` (or any role but write and
admin), the pages leave out every form that posts and every link to a page
that is only such a form; those pages, asked for, say "View only", and
books not yet opened are not offered to open. The company's details show,
disabled. Test: `tests/web.rs` `a_viewer_is_offered_no_change`, which
crawls every page as a viewer.

One GET abo does check: **Download the books** (`/export`), for the gateway's
admin role alone, or anyone where no gateway is in front; **Bring the books
up to date** (`/backups/import`) is admin's alone too, as is the
**Backups** page that holds both, and the download links. Admin means `X-Gateway-Role: admin`. A
request with no gateway header at all counts too, because then there is no
gateway in front: one person at their own computer. A user the gateway names with
no role does not count. The gateway makes no check of its own on a GET, so a
gateway sending the user without the role fails closed, not open. It is everything,
salaries included, and the gateway lets every role GET. Asked for by another
role it says "Admin only", and the sidebar leaves Backups out.
Test: `tests/web.rs` `the_books_download_whole`.

A script fetches the books through a **download link**,
`/export/<token>`, with `curl -fOJ <address>` and no sign-in: the address
is the key (§1's one exception). It is admin's to make and revoke, on
Backups, with a label, and with a last day, through which it answers,
or none. The page lists each link with who made it, when, and when it was
last used. That is the only sign of who uses it, since the key names no
one.
- The token is 256 bits from `/dev/urandom`, kept with its SHA-256 in
  `.abo/export-links.json`, outside git: not exported, not replayed, not
  carried by an import. It is kept so that Backups shows every link's
  address to an admin, whenever they look, as a private calendar address
  is shown. So whoever reads that file on the server, or is an admin,
  can read working addresses; anyone who can do either can read the
  books already. Links made when only the hash was kept show none.
- A wrong, revoked or expired token is answered as any page not there, a
  404, so tokens that once were cannot be told from those that never were.
  The route asks no gateway role: the key is the credential.
- abo's request log writes `/export/<link>` in its place, and the link's
  label and id. The page showing the address is `no-store`. The gateway's logs,
  and every proxy's, are theirs: a key in a URL is written down wherever
  it passes. A link may be given a last day; by default it has none and
  answers until revoked.
- Behind a gateway, the gateway forwards them without a session (its contract,
  item 12), as the gateway's config for abo declares in `token_paths`
  and `token_post_paths`, with `export` in `reserved`. Only these
  qualify: a GET or HEAD of exactly `/export/<64 lowercase hex>`, or of
  `/export/<64 lowercase hex>/info/refs?service=git-upload-pack` with
  that query and no other, and a POST of `/export/<64 lowercase
  hex>/git-upload-pack`, its body at most 1 MiB; the last two are git
  fetching (§11). The gateway
  strips every client `X-Gateway-*` header, cookie and `X-Forwarded-*`,
  sends `X-Gateway-Role: anonymous`, and allows 10 a minute per address.
  Anything else goes through the gateway's login as before.
- Backups lists the links in a table, one `tbody` each, lit under the
  pointer: label, who made it (the name a commit made then would carry),
  when, its last day, when it was last used, Revoke, and under them its
  address with a Copy button. Made, it is back to Backups, the link's id
  in the URL and never its key. The page is `no-store`. The address shown
  is the one a browser outside reaches,
  `https://<domain>/<owner>/<project>/export/<token>`, built from the gateway's
  `X-Forwarded-Proto`, `Host` and `X-Forwarded-Prefix` (without a gateway,
  this server's).

Every answer of the books whole is `Cache-Control: no-store`, so no proxy
or browser cache keeps a copy. A cross-site page can make a signed-in
browser fetch it, but it cannot read the answer (abo sends no CORS
headers). It costs the server a bundle each time. Admin only, that is
accepted.

The CSRF check above stays abo's: it guards a person's own browser against
other pages, with or without a gateway in front.

---

### Behind a gateway: what the gateway's config for abo must say

The gateway serves abo from a config of its own, kept with the gateway's deployment,
not in this repository. These lines are abo's requirements of it; without
them the pages behind a prefix, the download links and Backups break.

```toml
# Every top-level route abo serves (the gateway's contract, item 4), so that the gateway
# adds the project's prefix to abo's root-relative links and sends these
# paths to abo. A route added to abo's router is added here.
reserved = ["account", "agreement", "agreements", "apple-touch-icon.png", "backups", "bill", "bills", "chat", "company", "contracts", "declarations", "entry", "export", "icon-192.png", "icon-512.png", "import", "invoice", "invoices", "log", "manifest.webmanifest", "new", "partner", "partners", "pay", "post", "reconcile", "report", "scan", "undo", "write-off"]

# Download links (the gateway's contract, item 12): reached with no session, the
# token being the credential, which abo checks. The second entry of token_paths
# (a GET, its query exact) and the POST of token_post_paths are git fetching
# through the link (§11).
token_paths      = ["/export/{token}", "/export/{token}/info/refs?service=git-upload-pack"]
token_post_paths = ["/export/{token}/git-upload-pack"]
```

`tests` hold the list to abo's router: `reserved_routes_are_documented`
fails when a route is added to abo and not here.

What the gateway does with them is the gateway's, and abo needs none of it: a token
link's request comes with `X-Gateway-Role: anonymous` and nothing else of
the gateway's; a wrong token is abo's 404, and the gateway may turn a 404 for a
project that does not exist into its login redirect, so that a stranger
cannot tell which projects exist.

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
  `accounts.journal`, `.gitignore`, `git init`, first commit). `abo add` (now `abo post`) with the
  full validation chain. `abo balance` / `abo register` passthrough. One test: a balanced
  entry commits; an unbalanced one is rejected; an input-VAT posting under
  `method = "flat"` is rejected. ~300 lines. **Done.**
- **M1** — `abo bill <file>`: AI proposal, terminal approve/refine. **Done**,
  and since taken out of `abo` (§5): `abo file` and `abo bill` are model-free.
  The model's program, `scan` and `chat`, is its successor, beside `abo` (§5).
- **M2** — `abo-web serve`: balance, register, a booking form, and the bill upload
  and proposal card. **Done.** `abo-web demo` serves the same pages against a
  throwaway repository of toy data in the temp directory, rebuilt on every
  start, through the ordinary validated `post`.
- **M3** — invoices: typst template, numbering from the journal. **Done.**
  `templates/invoice.typ` is seeded into the books on first use and committed;
  it belongs to the company, not to abo. abo writes `.abo/invoice.json` and runs
  `typst compile`; the template does layout and never arithmetic, so rounding
  has one home. The next number is `max(invoice:) + 1` read from the journal —
  no counter file. The PDF is `git add`ed before the entry is committed, so both
  land in one commit or neither does.
- **M4** — reconciliation and statements. **Done.** `abo reconcile` / `abo
  record` / `abo unrecord` and a page of checkboxes; `abo pl`, `abo bs` on screen,
  as `--json`, and as PDFs through a second typst template. Bank import is
  not built (§12) — the point of reconciliation is to notice what is wrong, and typing
  a closing balance is enough to do that.
- **M5** — companies and users. `abo-web serve` serves one company: its books,
  or an empty directory, where it serves the form that opens them in it
  and then the company. A server for several companies under `/c/<dir>/`
  was built and taken out: behind a gateway a project is the unit of sharing,
  so one company per project makes a role on it a role on one company's
  books. Users are the gateway's: the author of a change is the person the gateway
  names (`X-Gateway-User`, `-Name`, `-Email`).
- **M6** — PWA manifest, or the JSON API if native has been chosen. **Done**
  as a manifest, one per company: relative URLs, so under a prefix
  (the gateway's `/<owner>/<project>/`) its scope is that company's alone. Chrome finds it installable
  (`tests/installable.mjs`). **Open:** adding it to a real phone's home
  screen, and on an iPhone whether a PDF opened in the app leaves a way back
  (WORKFLOWS.md G1).

- **M7** — the model beside abo. **Done.** `scan` and `chat` at a
  terminal (§5); the web interface moved out of abo into `abo-web`, its own
  binary (§3), which shows Chat and Scan where the model's program is installed.

Reconciliation was not in the original list of workflows and is the one that
recurs most: bills arrive a few at a time, the statement arrives all at once and
has to tie out.

---

## 11. Wish list

Wanted, but not specified closely enough to be a milestone yet. New entries go
here first; they graduate to §10 once the shape is clear. Statements and their
PDFs started here and are now §6 and M4. Payroll and bank import were here,
and are not on the roadmap (§12).

### Export, import and a git mirror

Requested 2026-11-04. The books are a git repository, so each of the three is a
git operation; what is missing is a way to do it from the browser, where
whoever keeps the books has no shell.

- **Export.** Built: **Download the books** on Backups, admin only
  (§8). Download the whole repository as one file: `git bundle create -
  --all`, which is the books with their history. A tarball of the working tree
  would drop the history, and the history is the audit trail (§1). The log
  is in the commits (§7), so it goes too. What does not: `.abo/`, typst's
  scratch files and the lock, and the git identity `abo init --git-name`
  wrote into the books' `.git/config`, which is who works on them here and
  not part of them. Changes not committed (WORKFLOWS F8) hold an export up,
  as they hold every change.
- **Import.** Built (WORKFLOWS A8), from such a file, two ways: on the form
  of A1 on an empty directory, `git clone` in place of the questions; and,
  admin only, on Backups, a newer copy brought in only as a fast
  forward. Diverged, nothing is brought in, and the page lists each side's
  own changes and gives, for whoever runs the server, the `git merge`
  that puts them together by hand: abo does not merge books. Nothing is
  taken on trust. The clone is read as any books are, against every
  invariant (MODEL.md §4) and hledger, with only plain files (a link could
  serve a file outside the books), and refused, and removed or reset, if it
  fails. From a URL to clone: not built; add it when books move between
  machines that can reach each other.
- **Mirror.** Built. A download link's address is also a git remote,
  so that the books are mirrored on another machine by git alone:

  ```sh
  git clone https://<domain>/<owner>/<project>/export/<token> books
  git -C books pull --ff-only
  ```

  abo answers git's smart HTTP protocol at that address, read only: the
  two requests a fetch makes, `GET <link>/info/refs?service=git-upload-pack`
  and `POST <link>/git-upload-pack`, each answered by `git http-backend`
  over the books, which inflates a gzipped request and speaks every
  protocol version; receive-pack is off and has no route, so nothing is
  pushed. The origin check (§8) passes the POST: its key is in the
  address, not a cookie a page could make a browser send. The token is
  checked as for the bundle, and it is the credential, so a revoked link
  stops the pulls too. What a fetch gets is what the bundle holds: the
  commits, with changes not committed (WORKFLOWS F8) holding it up as they
  hold an export. A plain GET of the link still answers the bundle.

  When and how often the other machine pulls is its own business, not
  abo's. Pulling with `--ff-only` refuses books rewritten on the server
  rather than copying them over the copy. Nothing leaves the sandbox, the
  server keeps no credential beyond the token, and a server taken over can
  serve bad commits but cannot change the copy.

  The gateway forwards two more token paths (§8), one of them a POST,
  which its contract (item 12) has to allow without a session. A fetch is
  two or three requests, inside the 10 a minute. Test: `tests/web.rs`
  `a_download_link_is_its_own_key` clones, pulls, and is refused a push.

---

## 12. Explicitly not built

Each with the trigger for revisiting.

| Not built | Add when |
|---|---|
| Account / commodity editor UI | You are editing `accounts.journal` weekly. It is `$EDITOR` plus `hledger check accounts`. |
| Database | Never, for the books. Users are the gateway's (§8). |
| Client-side JavaScript beyond small touches | A workflow genuinely cannot be a form submission. Three small touches so far, each with the page working without it: a form's Save button lit once a field differs (`EDITS`); a file dropped on a form taken as chosen (`DROP`), since a file input alone takes a drop only on itself; and a download link's Copy button, since the clipboard is the browser's alone. |
| Bootstrap, Tailwind, or any CSS framework | The hand-written stylesheet stops fitting on one screen, or a component needs behaviour rather than style. |
| An icon font or an icon package | You need more than about twenty icons. |
| JSON API | A native phone client is chosen (§7). |
| Dated / versioned VAT config | The company switches methods and an old period must be reproduced. |
| Currency conversion | Never at booking time. For reporting, `hledger -X`. |
| QR or structured payment codes on invoices | A customer asks for one. It is a template change plus a QR crate, not an abo change. |
| Statutory rates in code | Never. They are `[invoice] vat_rate` and `--vat-rate`. |
| Auth in the application | Never: the gateway's (§8). |
| Templating engine, ORM | Not foreseen. The workspace's three crates are §3's. |
| A third typst template before it is needed | A document abo makes that is neither an invoice nor a statement. |
| Payroll: gross to net, deductions, payslips | Not on the roadmap. A salary is a posting template on the employee's agreement: gross, what is withheld, net, the same entry each month. Agreements hold invoice templates today, not posting templates; until they do, it is booked by hand from the payslip. |
| Bank import | Not on the roadmap. Banks' exports vary too much to fix a format for them now. Reconciliation is a closing balance typed in and postings ticked (§6). |
| Tax return | Not on the roadmap. Each country has its own form, and a box is often not a balance but a rate of one, a sum, or an adjustment no account holds. Boxes configured as queries covered only the balances. The bookkeeper fills in the form from `abo pl` and `abo balance`. |
| Committing rendered statements | A filing needs archiving. Today they are handed to you. |
| Closing entries at year end | A balance sheet needs to show a closed prior year. Today unclosed profit is a computed equity line. |

