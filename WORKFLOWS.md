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

### A1. Open the books — Supported (CLI, browser)
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
was wrong: a currency must be three letters or more (`eur` is taken as EUR), a VAT method
may be the start of one (`e`, `f`, `n`), a rate must be a number from 0 up to
100. Refused before any question is asked: a
directory of that name already there, a name with no letters or digits,
`--dir`, and running it inside a company's books. The accounts the payment
options name are declared along with the default ones.
With any flag, `abo init` asks nothing: `--address` (a line each),
`--country`, `--tax-id`, `--currency`, `--vat`, `--vat-input`, `--vat-output`,
`--flat-rate`, `--flat-account`, `--sales-rate`, `--git-name`, `--git-email`,
and `--payment '{ kind = "bank", iban = "..." }'` per payment option. What is
not given is what the question offers; what is given is checked as a typed
answer is, and a wrong one is refused.
In the browser, `abo serve` on a data directory offers the same questions as
one form at `/new`: both are drawn from one description of them
(`src/questions.rs`), so they come in the same order, with the same checks and
the same defaults for a blank answer, and the same answers open the same books. The fields of a payment option's kind and of the
VAT method show once it is chosen (CSS, no script); another payment option
only when asked for with its button. Each wrong answer says why under it and
the form comes back as filled in; sent twice, it opens one company. A flag
no question asks for with the other answers — a sales rate under `--vat
none`, a network on a bank option — is refused, not dropped; and input that
runs out mid-way stops init, naming the question it ran out at, rather than
taking the rest as blank; on a terminal, Ctrl-D on an empty line is a blank
answer, refused where one is needed. A failure, the first commit's
included — no git identity, say — leaves no directory behind.
**Done when** the company's directory is a git repository with one commit
holding `abo.toml`, `journal/main.journal`, `journal/accounts.journal` (the
default accounts, plus the VAT accounts the method needs) and `.gitignore`,
and the first line of `abo log` is the `abo init` with flags that opens the
same books without asking.

### A2. Declare an account or a commodity — Supported (editor, by design)
Add an `account …` or `commodity …` line to `journal/accounts.journal` and
commit it. There is deliberately no editor for this in abo (DESIGN §12).
A commodity is declared with its places: `commodity 1000.00 EUR`, two.
**Done when** the new account or commodity can be posted to; an undeclared
one is refused with a message saying where to declare it (a commodity that
differs from a declared one only in case, `eur`, asks whether EUR was meant),
and so is an amount with more places than its commodity has. A declaration
counts once it is saved; committing it is for the record. A change left
uncommitted, staged or not, is never swept into a commit abo makes; an undo
waits until nothing is staged.

### A3. Start the web interface — Supported
`abo serve [--dir x]` serves one company when `x` (default: the working
directory) is inside its books; otherwise `x` is a data directory, and the
front page lists the companies in it, each name a link to that company's own
pages under `/c/<its directory>/` — the same pages, every link and form
among them kept under that prefix, with **All companies** in the sidebar to
come back. A missing `x` is created after a y/n
question that counts down five seconds and takes silence as no. `abo demo`
serves throwaway toy books. A missing `git` or `hledger` stops startup with a message naming
what it is for; a missing `typst` is a warning. The server reads `abo.toml` and
the journal afresh on every request, so an edit to either — a closed period, a
new rate, a declared account — applies without a restart.
**Done when** the front page shows the company, its balances and recent
entries; for a data directory, the name and directory of each company in it,
and a button to open the books for a new one (A1); and a change made in one
company's pages lands in that company's books alone. A directory that holds
no books, or a name that is no directory init would make, is "not found".

### A3b. Change the company's details — Supported
Name, address, country and tax registration number: what invoices print about
who sends them. The company page (the company's name in the sidebar) shows
them as init's questions, filled in, and saving runs `abo company --name ...
--address ... --country ... --tax-id ...` with only the details that changed.
`abo company` alone prints them. A bank option held in the company's old name
is held in its new one, in the same commit. The directory keeps its name; currency and
VAT method are shown there, not changed there. Payment options are below, see
A4.
**Done when** a change shows on the next invoice, is one line in the log, and
undo puts it back.

### A4. Change where invoices ask to be paid — Supported (browser, CLI)
A payment option is one of three kinds. A **bank** option has an account
holder, an IBAN (its check digits checked, and kept in groups of four), a BIC
and the bank's name and address; it needs an IBAN or, for places without
IBANs, further lines such as account and routing numbers. A **crypto** option
has a network and a wallet address, both needed. A **cash** option has lines
saying where and when. Each also has an id for commands (`postfinance`:
lower-case letters, digits, dashes), a name for invoices, further lines, and
the declared account the money arrives in, an asset (by kind `Assets:Bank`,
`Assets:Cash` or `Assets:Crypto`).
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
bad IBAN or BIC, a field of another kind, too little to pay by (a cash
option without its lines among it), a control character, an undeclared
account or one that is no asset, and an edit that changes nothing are refused; asked on the terminal,
the question comes again. Each change is a commit to `abo.toml`, comments
kept, logged and undoable like any other. The company page lists them, each
with Edit and Remove, and a button to add one: the same questions as a form,
each problem under its field, saving by the same commands.
**Done when** the next invoice prints the options as they now stand.

---

## B. Everyday booking

### B1. Book an entry by hand — Supported (browser, CLI)
Browser: **Entry**, fill in date, description and one posting per line
(`account=amount`, one amount may be left off). CLI: `abo post`.
**Done when** the entry is in the year's journal file in date order, one commit
holds it, and the front page says **Booked** with that commit. An entry that
does not balance, names an undeclared account, posts to a VAT account the
method forbids, or falls in a closed period is refused with the reason, and the
form comes back filled in; so is one in an undeclared commodity or with more
places than its commodity has (A2). Sending the same form twice — a double
click, a reload — books it once.

### B2. Book a supplier bill from a document — Supported (browser, CLI)
Browser: **Book a bill**, choose a PDF or take a photograph, give the date on
it, who issued it, a description and the postings, **File and book it**. CLI:
`abo file <scan> --date <date> --issuer <name>`, which says where it filed the
document, then `abo bill --desc <text> <that path> <postings>`.
**Done when** the document is filed as
`bills/<year>/<date>-<issuer>-<hash>.<ext>` in one commit, and the entry,
dated as the document, carries `bill:` and `sha256:` tags pointing at it in
the next. In the browser an entry the validator refuses files nothing, and
the form comes back filled in but for the document. A document filed and not
booked — with `abo file`, or left by a booking undone — is listed under the
form as **Filed, not booked yet**; **Book it** opens the form with that
document, and booking it files nothing again.

### B3. The same bill twice — Supported
Filing a document already filed is refused, naming where it is; booking a
document already booked is refused, naming the entry that holds it.

### B4. Record a payment or a receipt — Supported (as B1; bills in the browser)
Paying a bill, moving cash: an ordinary entry (B1) between the bank or cash
account and payables or receivables. A payment names the bill it pays by
its path, as a payment of an invoice names the invoice: `pays:` for a part,
`settles:` for the payment that settles it.
Browser: **Bills** lists every filed document, and every one a booking names
that the books lack, newest first. Each shows its amount, what its booking
credits (`?` where bookings disagree), and what is open: what it credits to
a liability less what payments naming it alone took off, blank once
settled or when it credited no liability; `?` where a payment names it
with other bills, which abo does not split. Bills not booked or still owed
come first, and what is open is summed in each commodity. Pay books a
payment of it, Upload files a document the books lack, Delete undoes the
filing of one not booked. The bill's page lists every entry that names it.
Reverting a bill's booking cancels the bill: the reversal is tagged
`cancels:` it, and a bill some payment names is not cancelled until that
payment is reverted. Reverting a payment or a cancelling takes it back: the
reversal is tagged `takes-back: settles <what>` (or `pays`, `cancels`), for
bills and invoices alike, and reverting that puts it back. A bill payment
pays off the bill's liability, no more than is open (where that is `?`, no
more than the bill credited), and the one that settles it all that is open
— a part that would leave nothing open is refused, as it should be tagged
`settles:`; it is dated no earlier than the bill. A bill is
booked once: a second booking naming it is refused, naming the first. A
cancelling moves the opposite of the booking. Undo refuses an entry reversed since, and a bill or
invoice that other entries still name. Filing a missing document for a
booking moves every tag naming it and records its hash, in one commit.
**Done when** the payable or receivable it settles goes to zero, and shows
as `0.00` on the front page.

### B5. Record a customer's payment of invoices — Supported (browser, CLI)
A payment names the invoices it pays by tag: `pays:` for part of one,
`settles:` for the payment that settles it. One payment may tag several
invoices, each either way, and an invoice may be paid in any number of parts.
Browser: **Invoices**, **Pay** on an invoice: a page for the payment of that
invoice, naming it and what has been paid on it, asks whether this payment
settles it (the default) or pays part of it, then the date, what arrived and
into which account. The amount starts at what is still owed when that is
known — the total less every payment tagging this invoice alone — and blank
when it is not: a payment of several invoices is not split between them. A payment that also
pays other invoices opens **Also pays other invoices** and says, for each not
yet settled, whether it pays part of it or settles it. CLI: `abo post ... --tag
pays=<number> --tag settles=<number>`.
**Done when** the entry takes the amount off the receivable into the account
chosen, carries one tag per invoice, and **Invoices** shows each as part paid
or settled. Refused: a tag naming a number the books have no invoice for, an
invoice already settled or cancelled, or an invoice named twice; tags on an
entry that takes nothing off the receivable; and on the Pay page, an amount
that is not above nought, or the receivable as where the money arrived.

### B6. Settle an invoice without a new payment — Supported (browser, CLI)
For an invoice whose payment is booked without saying so, or was settled
where the books do not show. **Invoices**, **Settle**: choose the booked
payment (an entry taking something off the receivable) and it is tagged
`settles:` that invoice, a `pays:` of it in that entry becoming `settles:`;
or, without a payment, an entry dated as given that moves nothing and says
only that the invoice is settled. CLI: `abo settle <number> --by <key>`, or
`abo settle <number> [--date ...]`.
**Done when** the tag is in the journal in one commit, logged and undoable,
and the invoice shows as settled. Refused: an entry that takes nothing off the
receivable, one in a closed period (settle without a payment instead), and an
invoice already settled or cancelled.

---

## C. Invoicing

### C1. Issue an invoice — Supported (browser, CLI)
**Issue an invoice**: customer, one line per item
(`quantity|unit price|description`), optionally date, VAT rate, commodity,
the customer's tax number (printed under their address), why no VAT is
charged, which payment options it prints, their reference and a note. CLI:
`abo invoice`, with `--customer-tax-id`, `--vat-reason` and `--pay-to <id>`,
repeatable. With no option chosen it prints them all, or for an invoice in
another commodity than the company's, its crypto options if there are any. With
none it says only when to pay; with one or more, "to the following" or "to
any of the following" and each option's name and details, side by side.
An invoice is stated to the places its commodity is declared with (A2) unless
`--decimals` says otherwise.
**Done when** the invoice is numbered one past the highest `invoice:` tag for
its year, its PDF is under `invoices/<year>/`, the receivable, income and (under
the effective method) output VAT are booked, and the PDF and entry are in one
commit. VAT follows the configured method: none charged when not registered,
the gross booked to income under a flat scheme. Sending the form twice issues
one invoice. A failure — typst missing, say — leaves no commit, no files and
the number unused. Refused: an invoice at 0% VAT, from a company that is VAT
registered, that does not say why; one dated before the invoice numbered
ahead of it (numbers follow dates), or after today (dated ahead by mistake,
it would hold every later one back); and one in another commodity charging VAT
when the books have no price for it in the company currency on or before its
date — the VAT is printed in the company currency too, with the price used.
Refused as well: an undeclared commodity; a VAT rate outside 0 to 100, or any
rate at all when the company is not VAT registered; a line that comes to
nothing at the invoice's places; and a total that is not above nought (to
take an invoice back, a credit note, F5). Issued with a field the books' own
template does not print — one copied in before the field existed — it says
so, naming the field, on the terminal and in the notice.

### C2. Find an issued invoice again — Supported (browser, CLI)
**Invoices** lists every invoice and credit note, newest first: number, date,
customer, total, due date, status, and a **PDF** button that downloads it —
greyed out, saying why, when the books lack the PDF. Clicking anywhere else
in a row opens the invoice's booking. The status
is one mark per entry that tags the invoice, in order, each dated and a link
to that booking: "part paid" for each `pays:`, "settled" for the `settles:`,
"cancelled" for the credit note's `credits:`; with none, "open" (or "credit
note" for one). An open or part-paid invoice past its due date is also marked
overdue. **Not settled** shows only open and part-paid ones. CLI: `abo
invoices [--open] [--json]`, the same marks on one line, newest first (the
JSON in number order). Pay and Settle refuse an invoice already settled or
cancelled. Status is read from the tags alone, never from amounts. An
invoice's own page lists what paid, settled or cancelled it, and what is still
owed where that is known; a `pays:`, `settles:` or `credits:` tag in any
entry's journal text links to the invoice it names.
**Done when** each invoice shows the marks its tags give it. An invoice
issued before abo kept what it printed is due its date plus `[invoice]
terms_days`. One paid on the spot, booking nothing to the receivable, is
settled on its own date, needing no tag.

### C3. Invoice in another commodity — Supported
The commodity field (`--currency`); its places are those it is declared with,
`--decimals` to say fewer: the invoice is rounded and printed to those, and
the entry written, as every entry is, to the places declared.
**Done when** the entry and the PDF are in that commodity, the PDF to the
places asked for.

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
Refused: an account that is not declared or not an asset, an undeclared
commodity, a balance with more places than its commodity has, clearing a
posting to another account or in a closed period, clearing what is cleared
already, and recording a statement
recorded already — but for recording it again, asserted, once what was
outstanding at the first recording has cleared.

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
**Log**: every change to the books, newest first, as the `abo` command that
made it, with every default it used written out; reads and refusals change
nothing and are not there. Each opens to the command and the diff its commit
made, documents shown as binary. The last 50 show, and **Show all** the rest.
A commit undone since is struck through and says so. CLI: `abo log`.
**Done when** the last change appears as a command that, typed, would make it
again.

### F2. Undo something just done — Supported (browser, CLI)
Every operation that succeeds says so in a green notice that fades after
ten seconds (hovering holds it), with **Undo** on it. The **Log** offers
**Undo** on every line, greyed out where it cannot: the opening of the books,
a filed document that is booked, something already undone, a change that
something since has changed the same lines of, or one in a closed period. CLI:
`abo log`, whose lines that can be undone say `(undo)`, then `abo undo <line>`.
Undoing a booking, a filing, an invoice, a clearing or a recorded balance
takes its change back out of the books — the entry, or the document — as a
new commit, so git keeps both. An undo is itself undoable.
**Done when** the books are as they were before the operation and the Log
shows it struck through; undoing the undo brings it back, and it is no longer
shown undone. Undoing the first entry of a year takes its emptied year file
and `include` line with it, and undoing that puts both back. Refused, with
nothing changed, when something since has changed
the same lines — the refusal names that line of the log, to undo first, never
one undone since — when it would change a closed period, or when it would
leave a bill both settled and cancelled, or paid more than it owes. Undoing an
invoice frees its number for the next one, so only the last numbered may be
undone — no gap is left — and only one not yet sent.

### F3. Un-tick a transaction cleared by mistake — Supported (as F2)
Undo the clearing.

### F4. Take back a filed document — Supported (as F2)
Undo its `abo file` line, once nothing books it.

### F7. Revert a booking — Supported (browser, CLI)
For a booking that stands and turns out wrong, the bookkeeping way: click it
among the recent entries to open its page — the entry as it is in the journal,
its invoice or reversal if any, and the commit that booked it — and
**Revert**. CLI: `abo revert <file:line:date>`, dated today unless `--date`
says otherwise.
**Done when** a new entry dated today carries the opposite amounts and a
`reverses:` tag naming the commit that booked the original, which stays where
it is. A booking already reversed says so and cannot be reversed again; nor
can one be reversed with a date before its own. An invoice is refused, in the
browser and the CLI alike, with the `abo credit` that corrects it (F5), and a
credit note is refused: both are documents the customer has.

### F5. Correct an issued invoice — Supported (browser, CLI)
An issued invoice is a document the customer has; it is corrected by a credit
note that cancels it in full, and a new invoice. Browser: open the invoice's
booking from the recent entries and **Correct**: a credit note is issued,
dated today, and the invoice form opens filled in as the cancelled invoice
was, to change and issue. CLI: `abo credit <number>` (`--date`, `--note`),
then `abo invoice` as usual.
**Done when** the credit note has the next number in the invoice series, its
PDF under `invoices/<year>/` says "Credit note", names the invoice it cancels
and its date, and repeats its lines with the quantities and amounts negated;
its entry negates every posting of the invoice's and is tagged `invoice:` with
its own number and `credits:` with the invoice's; PDF, its data and the entry
are one commit, and the receivable and output VAT of the invoice net to zero.
The corrected invoice is numbered after the credit note. An invoice issued
before abo kept what it printed (`invoices/<year>/<number>.json`) is
cancelled from its journal entry: one line, "Invoice N of date", with its net,
VAT and total, and the customer's name; the corrected invoice's form is then
filled in but for its lines. Refused: a number the books have no invoice for,
an invoice already cancelled or reversed, and a credit note. The invoice's
page then says what cancelled it and **Correct** is greyed out.

### F6. Close a period — Supported (config)
`[company] closed_through` in `abo.toml`, a quoted date:
`closed_through = "2026-06-30"`. Entries dated on or before it are refused, and
so is filing a bill dated then (it could never be booked).

---

## G. Phone

### G1. Photograph and book a bill on a phone — Partial
The pages work at phone width, and the upload offers the camera, the photo
library or a file.
**Gap:** no web app manifest yet, so it cannot be installed to the home screen
(DESIGN M6).

---

## H. Deferred

Listed so the list is complete; each has its trigger in DESIGN.md.

- **Users** (M5, the rest) — who did what, per person, and who may open which books. More than one set of books is served already (A3).
- **Bank import** (§11) — statement lines matched against booked items.
- **Payroll** (§11) — gross to net, payslips.
- **Year-end closing entries** (§12) — until then unclosed profit is a computed equity line.

---

## Last verified: 2026-10-01

Run twice that day, by four fresh agents each time, each with its own copy of
the demo books and its own server, typst installed, working from this file
and not the source: setting up (A, F6); booking, reconciling and reports
(B1–B3, D, E); invoicing and corrections (C1, C3, F5, F7, G1); bills,
settling, the Log and undo (B4–B6, C2, F1–F4). Every fault of the first run
held fixed in the second. What the second found was fixed the same day, with
a test for each in `tests/regressions.rs`, and not yet verified again.

| Workflow | Second run |
|---|---|
| A1, A2, A3, B1–B3, B5, C1, C2, C3, E1–E4, F1, F3, F5, F6, F7, G1 | Verified |
| A3b, A4 | Failed, fixed: `abo company` and `payment-options add` with flags took control characters; a cash option without lines was taken; money could arrive in an expense or income account. Also: init took VAT accounts of the wrong kind |
| B4, F2 | Failed, fixed: an undo could leave a bill part paid and cancelled, or paid and not booked; a payment of a bill whose open amount is `?` could be any amount; a `pays:` of all that was open left a bill that could never be settled |
| B6 | Failed, fixed: an invoice could be settled or paid before its own date |
| D1, D2 | Failed, fixed: a statement recorded could be recorded again, unasserted, at another balance; the browser's clear took an undeclared or other commodity, a balance that is no number, and any action |
| F4 | Failed, fixed: the undo of an undone Upload was refused, for the payment the upload had moved |
| (found writing a test) | Fixed: an undo checked the books before putting back a year file's `include`, so an entry it put back in another year went unchecked — a payment of a bill no longer booked among them |
| (thought of, then tested) | Fixed: a link or key made before another entry of the same day was booked above it reached that other entry — keys now carry a fingerprint of their entry; a commit swept up whatever else was staged; an invoice could be dated ahead, holding every later one back; an invoice below the last numbered could be undone, leaving a gap; zero-width and right-to-left characters were taken in text; one company's form token was taken as sent in another's; `abo settle --by` took a reversed payment, or one dated before the invoice; a document could be filed for a booking in a closed period, rewriting its tags; an asset declared in lower case was not offered to pay from; a description could make the Log take a commit for undone; a key could reach a twin entry that differed only in its tags; one inconsistency made by hand refused every later undo; an `abo` at a terminal and the server could write the books at once, colliding on git's index — the books are now held for one change at a time; a clearing could be undone under a statement recorded since; `abo.toml` edited by hand was not checked (a payment id twice, a receivable that is no asset, an account not declared); abo wrote into a year file holding someone else's uncommitted change; a log taken under laxer rules could not be replayed — each line now names its rules, and a replay takes it under them |
| (in passing) | Fixed: an invoice line without a description; a nought total refused in the wrong words; a credit note of an invoice paid on the spot shown positive, its VAT as `-0`; `abo post` and recorded statements echoing amounts as typed; `reconcile --help` saying `--record` always asserts |

Known, not fixed: error pages answer 200, and a direct POST missing a field
gets a bare 422; at 0% a BTC invoice prints a VAT line where EUR does not;
unit prices print as typed, finer than the commodity if typed so; the same
IBAN may be added twice; the Log offers Undo on a line that the bill check
would then refuse; hledger reads `2026-13` as 2026, and abo with it; a
typst error reaches the terminal with its box-drawing characters; a
reversal, and the opening balance, can be reverted.

Not yet done: users and logins (the rest of M5), and a web app manifest
(M6, G1).
