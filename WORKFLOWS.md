# Workflows

What a person using abo sets out to do, and what counts as done. DESIGN.md says
how abo is built and why; this file says what it is for. Each workflow is
written so that someone who has not read the code can try it and tell whether
it worked: the steps name what is on screen, and **Done when** names what must
then be true of the books.

Status is one of:

- **Supported** — works in the interfaces listed.
- **Partial** — works, with the gap stated.
- **Missing** — wanted, not built; the decision still open is stated.
- **Deferred** — in DESIGN.md §11 or §10, with its trigger; listed so the list is complete.

Verification is by a fresh agent that has read this file and not the source,
working a copy of the demo books through the browser and the CLI, and checking
the result against the journal and `git log`. "Verified" below records the
last such run.

---

## A. Setting up

### A1. Open the books — Supported (CLI)
`abo init <name>` (its words need no quotes), run in a data directory, makes the company's directory
there: the name spelled in ASCII and in lower case, every run of anything but
letters and digits made one dash (`Smith & Sons, LLC` → `smith-sons-llc`,
`Łódź Straße` → `lodz-strasse`). It then asks, one per
line: the company's address (a line at a time, ended by an empty line),
country, tax number, where invoices ask to be paid (payment options: a kind
from a list, or skip, then its questions, as in A4; after each, another only
if asked for with yes), currency and VAT method (chosen from a
list: up and down or a letter, then space); then what the
method needs — for `effective` the input and the output VAT accounts, for
`flat` the rate remitted and the VAT account, for `none` nothing — and, unless
the method is `none`, the rate charged to customers; and last git's
`user.name` and `user.email` for the new repository, offering the global
ones. Blank
takes the default shown in brackets. A wrong answer is asked again with what
was wrong: a currency must be letters (`eur` is taken as EUR), a VAT method
may be the start of one (`e`, `f`, `n`), a rate must be a number from 0 up to
100. Refused before any question is asked: a
directory of that name already there, a name with no letters or digits,
`--dir`, and running it inside a company's books. The accounts the payment
options name are declared along with the default ones.
**Done when** the company's directory is a git repository with one commit
holding `abo.toml`, `journal/main.journal`, `journal/accounts.journal` (the
default accounts, plus the VAT accounts the method needs) and `.gitignore`.

### A2. Declare an account or a commodity — Supported (editor, by design)
Add an `account …` or `commodity …` line to `journal/accounts.journal` and
commit it. There is deliberately no editor for this in abo (DESIGN §12).
**Done when** the new account can be posted to; an undeclared one is refused
with a message saying where to declare it.

### A3. Start the web interface — Supported
`abo serve [--dir x]` serves one company when `x` (default: the working
directory) is inside its books; otherwise `x` is a data directory, and the
front page lists the companies in it. A missing `x` is created after a y/n
question that counts down five seconds and takes silence as no. `abo demo`
serves throwaway toy books. A missing `git` or `hledger` stops startup with a message naming
what it is for; a missing `typst` is a warning. The server reads `abo.toml` and
the journal afresh on every request, so an edit to either — a closed period, a
new rate, a declared account — applies without a restart.
**Done when** the front page shows the company, its balances and recent
entries; for a data directory, the name and directory of each company in it.

### A4. Change where invoices ask to be paid — Supported (CLI)
A payment option is one of three kinds. A **bank** option has an account
holder, an IBAN (its check digits checked, and kept in groups of four), a BIC
and the bank's name and address; it needs an IBAN or, for places without
IBANs, further lines such as account and routing numbers. A **crypto** option
has a network and a wallet address, both needed. A **cash** option has lines
saying where and when. Each also has an id for commands (`postfinance`:
lower-case letters, digits, dashes), a name for invoices, further lines, and
the declared account the money arrives in (by kind `assets:bank`,
`assets:cash` or `assets:crypto`).
`abo payment-options` (or `... list`) shows them in the order invoices print
them. `... add` puts up a list: bank, cash, crypto or skip, chosen as the VAT
method is; then the questions for the kind chosen, each answer starting on the
line as most would give it (the company as holder, "Bank transfer" as the
name, the kind as the id). `... add <id> --kind ... --iban ... --bic ...
--bank ... --network ... --address ... --detail ... --label ... --account ...`
takes the parts as given. `... edit <id>` asks again, each answer on the line
as it stands, or changes just the parts given as flags (and `--id`); another
kind drops the old kind's fields. `... remove <id>` removes one. Wherever an
id is asked for, its start will do when it names one option. A taken id, a
bad IBAN or BIC, a field of another kind, too little to pay by, an undeclared
account and an edit that changes nothing are refused; asked on the terminal,
the question comes again. Each change is a commit to `abo.toml`, comments
kept, logged and undoable like any other.
**Done when** the next invoice prints the options as they now stand.

---

## B. Everyday booking

### B1. Book an entry by hand — Supported (browser, CLI)
Browser: **Entry**, fill in date, description and one posting per line
(`account=amount`, one amount may be left off). CLI: `abo add`.
**Done when** the entry is in the year's journal file in date order, one commit
holds it, and the front page says **Booked** with that commit. An entry that
does not balance, names an undeclared account, posts to a VAT account the
method forbids, or falls in a closed period is refused with the reason, and the
form comes back filled in. Sending the same form twice — a double click, a
reload — books it once.

### B2. Book a supplier bill from a document — Supported (browser, CLI)
Browser: **Book a bill**, choose a PDF or take a photograph, read the proposed
entry, **Approve and commit**. CLI: `abo bill <file>`, then `a`.
**Done when** the document is filed under `bills/<year>/`, the entry carries
`bill:` and `sha256:` tags pointing at it, and both are in one commit.

### B3. Correct a proposal before approving — Supported (browser, CLI)
On the proposal, type a correction in plain words ("that is hosting, not
general expenses") and send it; the model proposes again. Approval is blocked
while the model is asking a question or the validator objects, and the reason
is shown.
**Done when** the new proposal reflects the correction, and nothing was
committed before approval. While approval is blocked, the reason is given
beside the button; a refused approval or correction comes back on the bill's
own page. A correction with no words in it is refused without
asking the model.

### B4. Come back to a bill later — Supported (browser, CLI)
A proposal not yet approved waits. The front page lists it under
**waiting for you** and Bills shows a count; uploading the same document again,
or re-running `abo bill` on it, resumes the same conversation.
**Done when** the resumed proposal is the one left, with its corrections, and no
second request was paid for. The page says it was picked up from before, and
lists what you have told the model so far.

### B5. The same bill twice — Supported
Uploading a document already booked is refused before any request to the model.
**Done when** the refusal names the entry that already holds it.

### B6. Record a payment or a receipt — Supported (as B1)
Paying a bill, a customer paying an invoice, moving cash: an ordinary entry
(B1) between the bank or cash account and payables or receivables.
**Done when** the payable or receivable it settles goes to zero, and shows
as `0.00` on the front page.

---

## C. Invoicing

### C1. Issue an invoice — Supported (browser, CLI)
**Issue an invoice**: customer, one line per item
(`quantity|unit price|description`), optionally date, VAT rate, commodity,
their reference and a note. CLI: `abo invoice`, where `--pay-to <id>`,
repeatable, chooses which payment options the invoice prints; the browser, and
the CLI without it, print them all. With none it says only when to pay; with
one or more, "to the following" or "to any of the following" and each
option's name and details, side by side.
**Done when** the invoice is numbered one past the highest `invoice:` tag for
its year, its PDF is under `invoices/<year>/`, the receivable, income and (under
the effective method) output VAT are booked, and the PDF and entry are in one
commit. VAT follows the configured method: none charged when not registered,
the gross booked to income under a flat scheme. Sending the form twice issues
one invoice. A failure — typst missing, say — leaves no commit, no files and
the number unused.

### C2. Find an issued invoice again — Partial
`/invoice/<number>` serves its PDF, and the browser opens it right after
issuing; a number with no PDF is a plain "not found". An invoice among the
recent entries on the front page links to its PDF.
**Gap:** nothing lists all issued invoices; older ones are found in
`invoices/<year>/`.

### C3. Invoice in another commodity — Supported
The commodity field (`--currency`) and `[invoice] decimals` (`--decimals`).
**Done when** the entry and the PDF are in that commodity with its places.

---

## D. Reconciling

### D1. Reconcile an account with a statement — Supported (browser, CLI)
**Reconcile**: account, commodity, statement date and the closing balance on
the statement. Tick every outstanding item the statement shows and
**Mark ticked as cleared**; when the difference is nil, **Record this balance**.
CLI: `abo reconcile`, `abo clear`, `abo reconcile --record`.
**Show** updates the figures without changing anything.
**Done when** each ticked item's posting to that account carries `*` in the
journal — the posting, not the whole transaction, so a transfer ticked off
against the cash box stays outstanding against the bank — each marking is a
commit, and recording writes a dated entry, with a balance assertion when
nothing is outstanding and without one otherwise. After a marking or a
recording the page is reloaded fresh, so reloading it does not do it again.

### D2. Reconcile cash or a wallet — Supported (as D1)
Any asset account and any commodity.

---

## E. Reports

### E1. Profit and loss for a period — Supported (browser, CLI)
**Statements**, Profit and loss, a period (`2026`, `2026q1`, `last month`).
CLI: `abo pl --period …`, with `--json` or `--pdf`.
**Done when** income and expenses are listed with totals and the profit for
the period, matching `hledger balance` over the same period. Downloading a PDF
changes nothing in the books.

### E2. Balance sheet at a date — Supported
**Done when** assets, liabilities and equity are listed, unclosed profit shows
as an equity line, and the bottom line (assets less liabilities and equity) is
nil.

### E3. Tax return for a period — Supported once configured
Needs `[[report.vat]]` lines in `abo.toml` (DESIGN §6). Without them the page
says what to write.
**Done when** each configured box shows its figure for the period, in the
company currency. Amounts in another commodity are valued at the price on the
day of the posting, from `P` directives in the journal; with no price the
return is refused, naming the box and the directive to add, rather than
leaving the amount out.

### E4. Ask hledger anything — Supported (CLI)
`abo bal …` and `abo reg …` pass their arguments to hledger.

---

## F. Checking and correcting

### F1. See what was done — Supported (browser)
**Log**: every operation asked for, newest first, including refusals. Each
opens to its input and, for a commit, the diff it made. A commit undone since
with `git revert` is struck through and says so.
**Done when** the last action appears with the right input and outcome.

### F2. Undo something just done — Supported (browser, CLI)
Every operation that succeeds says so in a green notice that fades after a
couple of seconds (hovering holds it), with **Undo** on it. The **Log** offers
**Undo** on every line, greyed out where there is nothing to undo: a failure, a
correction, a PDF, or something already undone. CLI: `abo log`, then
`abo undo <line>`.
Undoing a booking, a bill, an invoice, a clearing or a recorded balance takes
its change back out of the books — the entry, and any document it filed — as a
new commit, so git keeps both. An undo is itself undoable.
**Done when** the books are as they were before the operation and the Log
shows it struck through. Refused, with nothing changed, when something since
has changed the same lines (a booking cleared since: undo the clearing first,
or revert the booking), or when it would change a closed period. Undoing an
invoice frees its number for the next one; undo only one not yet sent.

### F3. Un-tick a transaction cleared by mistake — Supported (as F2)
Undo the clearing.

### F4. Throw away a pending bill — Supported (browser, as F2)
**Discard** on the bill's page, or undo its upload in the Log.

### F7. Revert a booking — Supported (browser, CLI)
For a booking that stands and turns out wrong, the bookkeeping way: click it
among the recent entries to open its page — the entry as it is in the journal,
its invoice or reversal if any, and the commit that booked it — and
**Revert**. CLI: `abo revert <file:line:date>`.
**Done when** a new entry dated today carries the opposite amounts and a
`reverses:` tag naming the commit that booked the original, which stays where
it is. A booking already reversed says so and cannot be reversed again.

### F5. Correct an issued invoice — Missing
An issued invoice is a document the customer has; it is corrected by a credit
note, not by editing.
**Open decision:** whether a credit note is an invoice with negative lines and
its own number series, and how it refers to the one it corrects.

### F6. Close a period — Supported (config)
`[company] closed_through` in `abo.toml`; entries dated on or before it are
refused.

---

## G. Phone

### G1. Photograph and book a bill on a phone — Partial
The pages work at phone width and the upload opens the camera.
**Gap:** no web app manifest yet, so it cannot be installed to the home screen
(DESIGN M6).

---

## H. Deferred

Listed so the list is complete; each has its trigger in DESIGN.md.

- **Companies and users** (M5) — who did what, per person; more than one set of books.
- **Bank import** (§11) — statement lines matched against booked items.
- **Payroll** (§11) — gross to net, payslips.
- **Year-end closing entries** (§12) — until then unclosed profit is a computed equity line.

---

## Last verified: 2026-09-24

Four fresh agents, each with its own copy of the demo books and its own
server, the bill workflows against a stand-in for the model (`ABO_STAND_IN`,
DESIGN §5). No API key on that machine; typst for the last of the four.

| Workflow | Result |
|---|---|
| A1 | Verified after the fix: the documentation had left out the VAT-account questions |
| A2, A3, B1–B6, D2, E1, E2, E4, F1 | Verified |
| D1 | Verified after two fixes: clearing marked the whole transaction, and a resent form recorded twice |
| E3 | Verified after the fix: income in another commodity was silently left out |
| F6 | Verified after the fix: a running server ignored a newly closed period |
| C1, C3 | Verified, with PDFs read back and checked, after two fixes: line amounts were not stated to the commodity's places ("0.0005 BTC", "EUR 0"), and a refused line did not say which line or everything wrong with it. Without typst the failure is clear and leaves nothing behind |
| C2 | Partial, as documented; an unknown number now answers "not found" instead of an I/O error |
| Statement PDFs | Verified against the screen and hledger; the template is committed with the first one only |
| Any commit | Fixed in passing: a commit that failed (no git identity) left its entry in the journal, and the next commit swept it up — two invoices entered the books without PDFs. A failed commit now puts the journal back |
| G1 | Partial, as documented |
| F2–F5 | Missing, as documented |

Seen in that run and fixed since: a refusal now stays on its own page (the
bill, the upload form) or offers **Back** to the page it came from; a disabled
**Approve** says why beside it; a bill shows its corrections and says when it
was resumed; a settled account shows 0.00 instead of vanishing; the typst
warning comes only from commands that make a PDF; the Log marks a reverted
commit; a statement PDF no longer commits anything; and recent entries link an
invoice to its PDF.

On the invoice as a document, for whoever owns the template and the rules:

- An invoice in BTC still gives the bank details as the way to pay, and states
  its VAT in BTC only; VAT law usually wants it in the national currency too.
- A 0% invoice says nothing about why; the reason (export, exemption) has to be
  typed into the note.
- There is no field for the customer's tax number, which cross-border B2B
  invoices usually need.
- An invoice may be dated before the one numbered ahead of it; many places
  require numbers to follow dates.
