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
`reconcile`, `clear`, `revert`, `undo`, `company`, `payment-options`, `pl`, `bs`,
`vat`, `balance`, `register`, `log`, `serve`, `demo`. M5: `abo serve` serves
one company; users come from a gateway, in front of it.
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
| Deployment | localhost; behind a gateway when reachable | abo is not responsible for authentication: no login, no users, no TLS. The gateway, in front, is all three. See §8. |
| Tax | three scheme *shapes* in code (`none` / `flat` / `effective`); every rate, rule and name in config | The shape changes what a *correct booking* is, so the validator must know it. Rates and local rules differ everywhere and are data. See §6. |
| Commodities | multi-commodity from the start | hledger already is. Cash, bank and wallet balances are all just asset accounts. |
| Phone app | PWA over the existing server-rendered pages | See §7. |
| Questions | every command that asks takes `--ni`, and then asks nothing: each question takes what it offers, as a flag left out does, and a yes or no is no | A script, a model's tool call or the gateway must never wait on stdin. Under `--ni` a question reached all the same is an error, not a wait (`tests/regressions.rs` `ni_asks_nothing`). A new command that asks adds its case there. |
| `abo init` | opens the books in the directory it is run in, or `--dir`, which is there and empty | As `git init`. The web form does the same on an empty directory, so a gateway project is one company. |

### Crates

`clap serde toml serde_json axum tokio maud rust_decimal jiff sha2 anyhow
unicode-general-category`

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
src/main.rs       main: the programs abo runs, logging, then the CLI
src/cli/          the command line: clap's args, each command run, the terminal's questions
src/model/        the books as MODEL.md defines them: state, check, operations, text,
                  what a person may type (answer.rs), refusals said of the operation
src/store.rs      the books on disk: read through the model, change by an Op, commit, log
src/command.rs    an Op as the command that makes it, and as its commit's subject
src/input.rs      what a person types, read: dates, amounts, postings, entries by number
src/views.rs      what the books show, as data: invoice rows, reconciling, money
src/questions.rs  init's and a payment option's questions, for the terminal and the form
src/repo.rs       finding the books, git, the lock, slugs
src/statement.rs  P&L, balance sheet, tax return, from hledger
src/invoice.rs    issuing an invoice or credit note, and its PDF's data
src/pdf.rs        typst: a template and its data made a PDF
src/report.rs     hledger passthrough (-O json / -O csv)
src/demo.rs       the demo's toy books
src/web/          axum + maud: a module per page's area, the forms, the opening
                  form, and style.css
```

No `lib.rs`/`bin/` split, no workspace, no crate-per-module.

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

The web server calls these functions **in-process**. It does not shell out to the
CLI and it does not talk to itself over HTTP.

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

Not in `abo`. Reading a scanned bill and suggesting the entry is the kind of
help a model gives, and it belongs in a larger application around `abo`: one
that shows a person the `abo file` and `abo bill` it suggests, takes their
corrections, and runs the commands they settle on. `abo` then sees only those
commands, validates them as it would a typed one (§6), and logs them. Nothing
in the books depends on what a model said.

An earlier version called the Messages API from `abo bill` itself, with the
conversation kept in `.abo/`. It was taken out so that the backbone is
model-free; the design of that call is in git history if the larger
application wants it.

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
manual, and it needs no state of its own: hledger already says a posting is
unmarked, pending or cleared, so marking one cleared means writing a `*` in
front of its account in the journal, which is a commit like any other. It is
the posting that is marked, not the transaction: a transfer from the bank to
the cash box is seen by two statements, often on different days, and ticking
it off against one must leave it outstanding against the other. A `*` on the
transaction line, as older books may have, still counts for all its postings.

```
abo reconcile --account Assets:Bank --date 2026-04-30 --balance 24390.00
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

### Reports

`abo pl`, `abo bs`, `abo vat`, each with `--json` and `--pdf <file>`, and each a
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
- **`[report.vat.<code>]`** — the tax return, one table per box on the form,
  in the order of their codes. Each has a `label` and a `query` (any hledger
  query; its balance over the period is the figure), with `invert` for a box
  on the credit side. A box that is a percentage of another, or a sum of
  others (a net figure), is not built: when a form needs one, it is a small
  evaluator in `statement.rs`, and still not a rate in the code.

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
beside the "listening on" line for `serve` and `demo`, since that is where
someone starting a server actually reads. Better than a puzzling failure three steps into a
workflow, and for `abo serve` it means the server declines to boot broken
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
`/declarations` is Accounts, `/post` is Entry. Partners and agreements
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
subject, as `Abo: {"argv": [...], "what": "...", "entry": 3}`, one for each
operation it holds; the subject is put on one line first, so that no text
typed into it makes a line of its own. Its stamp is git's: the author name
and email, and the second the operation began, as both author and committer
date. An invoice PDF carries the stamp as its creation date. So the log goes
wherever the history goes — a clone, a bundle, a mirror — and undo there
takes back the last change as it would here. Books from before kept their
lines in `.abo/log.jsonl`; it is read first, as all of them are older.
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

What abo takes from a gateway is who is asking: `X-Gateway-User`, `X-Gateway-Name`
and `X-Gateway-Email`, the author of the commits a request makes. It trusts
them because only the gateway can send them; the gateway replaces any a browser sends.
A role abo does not check: the gateway refuses a viewer's POST before abo sees it.
It only shows it: given `X-Gateway-Role: view` (or any role but write and
admin), the pages leave out every form that posts and every link to a page
that is only such a form; those pages, asked for, say "View only", and
books not yet opened are not offered to open. The company's details show,
disabled. Test: `tests/web.rs` `a_viewer_is_offered_no_change`, which
crawls every page as a viewer.

The CSRF check above stays abo's: it guards a person's own browser against
other pages, with or without a gateway in front.

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
  `accounts.journal`, `.gitignore`, `git init`, first commit). `abo add` (now `abo post`) with the
  full validation chain. `abo balance` / `abo register` passthrough. One test: a balanced
  entry commits; an unbalanced one is rejected; an input-VAT posting under
  `method = "flat"` is rejected. ~300 lines. **Done.**
- **M1** — `abo bill <file>`: AI proposal, terminal approve/refine. **Done**,
  and since taken out of `abo` (§5): `abo file` and `abo bill` are model-free.
- **M2** — `abo serve`: balance, register, a booking form, and the bill upload
  and proposal card. **Done.** `abo demo` serves the same pages against a
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
  clear` and a page of checkboxes; `abo pl`, `abo bs`, `abo vat` on screen,
  as `--json`, and as PDFs through a second typst template. Bank import moved
  to §11 — the point of reconciliation is to notice what is wrong, and typing
  a closing balance is enough to do that.
- **M5** — companies and users. `abo serve` serves one company: its books,
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

### Export, import and a git mirror

Requested 2026-11-04. The books are a git repository, so each of the three is a
git operation; what is missing is a way to do it from the browser, where
whoever keeps the books has no shell.

- **Export.** Download the whole repository as one file: `git bundle create -
  --all`, which is the books with their history. A tarball of the working tree
  would drop the history, and the history is the audit trail (§1). The log
  is in the commits (§7), so it goes too. What does not: `.abo/`, typst's
  scratch files and the lock, and the git identity `abo init --git-name`
  wrote into the books' `.git/config`, which is who works on them here and
  not part of them. Changes not committed (WORKFLOWS F8) hold an export up,
  as they hold every change.
- **Import.** Open books from such a file, or from a URL to clone: the web form
  of A1 on an empty directory, with `git clone` in place of the questions.
  Nothing is taken on trust. The clone is read as any books are, against every
  invariant (MODEL.md §4) and `hledger check`, and refused, and removed, if
  it fails.
- **Mirror.** A git remote that abo pushes to after every commit it makes.
  A failed push never fails the commit or undoes it. The books are right
  locally, the next commit tries again, and the page says the mirror is
  behind. The remote is a setting in `abo.toml`; a credential is not. An SSH
  key or a token lives outside the repository, in `.abo/` or the
  environment, so that cloning the books never hands it out.

Open questions, to settle before this becomes a milestone:

- **Where the credential comes from under a gateway.** The sandbox (§8) has a
  network of its own, so a push needs the gateway to let it out, and the key is
  either the gateway's to hold or the project admin's to upload.
- **Who may export.** An export is everything, salaries included (Payroll,
  below). It is a GET, so the gateway's refusal of a viewer's POST does not cover
  it. Admin only, probably.
- **Whether import is worth building.** On the CLI it is `git clone`
  followed by `abo serve`. In the browser it is the way to move books
  between the gateway projects, and that need arises only when someone moves
  books.

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

Until then, payroll is booked, not computed (MODEL.md §10): an employee is a
partner, their contract an agreement under `contracts/` with its signed
copy, and each month's salary an entry booked by hand from the payslip —
gross, withholding, net.

Open questions, to settle before this becomes a milestone:

- **Who may read the repository.** Employees and their salaries live in
  `partners.toml` and `contracts/`, versioned with everything else — and so
  does every name and salary, in the repository, for whoever can read it.
  That is a §8 question, and it is the first thing in the design with a
  reason to be readable by some people and not others.
- **Progressive withholding.** A flat rate per deduction covers contributions
  but not banded income tax. Either a bracket table in the same config, or
  withholding stays entered per employee, as booking it does today.
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
| Database | Never, for the books. Users are the gateway's (§8). |
| Client-side JavaScript | A workflow genuinely cannot be a form submission. |
| Bootstrap, Tailwind, or any CSS framework | The hand-written stylesheet stops fitting on one screen, or a component needs behaviour rather than style. |
| An icon font or an icon package | You need more than about twenty icons. |
| JSON API | A native phone client is chosen (§7). |
| Dated / versioned VAT config | The company switches methods and an old period must be reproduced. |
| Currency conversion | Never at booking time. For reporting, `hledger -X`. |
| QR or structured payment codes on invoices | A customer asks for one. It is a template change plus a QR crate, not an abo change. |
| Statutory rates in code | Never. They are `[invoice] vat_rate` and `--vat-rate`. |
| Auth in the application | Never: the gateway's (§8). |
| Templating engine, ORM, workspace split | Not foreseen. |
| A third typst template before it is needed | `templates/payslip.typ` arrives with payroll in §11, not before. |
| Committing rendered statements | A filing needs archiving. Today they are handed to you. |
| Closing entries at year end | A balance sheet needs to show a closed prior year. Today unclosed profit is a computed equity line. |

