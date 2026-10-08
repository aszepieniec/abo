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
takes the default shown in brackets; without a terminal, where the default
cannot be put on the line to edit, every question that has one shows it so. A wrong answer is asked again with what
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
answer, and where one is not enough init stops there, saying what was
needed, rather than asking again as it does after Enter. A failure, the first commit's
included — no git identity, say — leaves no directory behind.
**Done when** the company's directory is a git repository with one commit
holding `abo.toml`, `journal/main.journal`, `journal/accounts.journal` (the
default accounts, plus the VAT accounts the method needs) and `.gitignore`,
and the first line of `abo log` is the `abo init` with flags that opens the
same books without asking.

### A2. Declare an account, a commodity or a price — Supported (browser, CLI)
Browser: **Declarations**, from the company's page: the accounts, the
commodities and the prices, each with Add and Remove. CLI: `abo account add
<Account>`, `abo commodity add <CODE> <places>`, `abo price add <date> <CODE>
<amount>`, each with `list` and `remove`.
**Done when** the new account or commodity can be posted to, as one commit.
Refused (MODEL.md §5.1): an account whose first segment names no class, one
under an account posted to, a commodity not of three or more capital letters
or of more than 8 places, a removal of one in use, and a price on or before
the closing date. There is no edit and no rename.

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
--address ... --country ... --tax-id ...` with every detail as it now stands,
an address line to each `--address`. Saved as it stood, nothing changes and
nothing is said.
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
answers the same questions from its flags (`--details` and `--bank` a
line each, as `init --payment` takes them a line or a list): what is not given is what they
offer, as a blank answer is (the company as holder, "Bank transfer" as the
name), a flag of another kind is refused, and every problem is said at once. `... edit <id>` asks again, each answer on the line
as it stands, or changes just the parts given as flags (and `--id`); another
kind drops the old kind's fields. `... remove <id>` removes one. Wherever an
id is asked for, its start will do when it names one option. A taken id, a
bad IBAN or BIC, a field of another kind, too little to pay by (a cash
option without its lines among it), a control character, or an account
that is no asset are refused; an account not declared yet is declared
along with the option, as init declares it; asked on the terminal, the
question comes again. An edit that changes nothing makes no commit. Each
change is a commit to `abo.toml`, written as abo writes it (a comment there is
a change no abo command made, and refused as one), logged and undoable like
any other. Invoices print the options in the order of their ids, as the list
shows them. The company page lists them, each
with Edit and Remove, and a button to add one: the same questions as a form,
each problem under its field, saving by the same commands.
**Done when** the next invoice prints the options as they now stand.

---

## B. Everyday booking

### B1. Book an entry by hand — Supported (browser, CLI)
Browser: **Entry**, fill in date, description and one posting per line
(`account=amount`, every amount written). CLI: `abo post`.
**Done when** the entry is in the year's journal file in date order, one commit
holds it, and the front page says **Booked** with that commit. An entry that
does not balance, names an undeclared account, or falls in a closed period is
refused (a VAT account may be posted to by hand: an entry by hand is how a
correction is made) with the reason, and the
form comes back filled in; so is one in an undeclared commodity or with more
places than its commodity has (A2), and one carrying a tag abo writes itself
— `reverses`, `takes-back`, `cancels`, `invoice`, `credits`, `bill`, `sha256`
— which only the command that keeps it true writes. Sending the same form twice — a double
click, a reload — books it once.

### B2. Book a supplier bill from a document — Supported (browser, CLI)
Browser: **Book a bill**, choose a PDF or take a photograph, give the date on
it, who issued it, a description and the postings, **File and book it**. CLI:
`abo file <scan> --date <date> --issuer <name>`, which says where it filed the
document, then `abo bill --desc <text> --document <that path> <postings>`.
Where the bill gives them, the supplier's own number for it and when it is
due go with it, either, both or neither: `--number`, `--due`, and the two
fields of the same names on the form (MODEL.md I-B3). A bill owing what
another owes under the same number of the supplier's is the same bill,
whatever their dates, and is refused even when said to be another (N8).
**Done when** the document is filed as
`bills/<year>/<date>-<issuer>-<hash>.<ext>`, its type read from
its bytes, and the bill is booked, dated as the document, the
document naming it in `bills/index`: in the browser one change, which one
Undo takes back (MODEL.md §5.4); in a terminal two. **File it, to book
later** on the form files the document alone, as `abo file` does. In the browser a bill the rules refuse
files nothing, and the form comes back filled in but for the document. A
document filed and not booked is listed under the form as **Filed, not booked
yet**; **Book it** opens the form with that document. A bill may be booked
with no document, and its document filed later (**Upload** on the bill, which
asks who issued it, or `abo file --for <bill>`). `abo bill --date`, given with
a document, must be the document's date.

### B3. The same bill twice — Supported
Filing a document already filed is refused; booking a document a standing
bill has is refused (MODEL.md I-B1); so is a bill owing what a standing
one owes, on the same account, dated within 7 days of it either side,
document or not and however it is split among expenses, unless it is said
to be another — `abo bill
--again`, or the checkbox on the form (N8). Under the same number of the supplier's
it is that bill whatever is said.

### B4. Pay a bill, or receive an invoice's payment — Supported (browser, CLI)
A payment names the one bill or invoice it pays, by its id; whether that is
settled is what the amounts say. Browser: **Pay** on a bill or an invoice: the
date, the amount (what is open, to start with; less pays a part), the money
account it is paid from or into, and, paid in another commodity, what was
paid. CLI: `abo pay <id or invoice number>` (`--amount`, `--from`, `--cost`,
`--date`).
**Done when** the payment takes the amount off the debt's account, the debt
shows as part paid or paid, and the payable or receivable and what is open
agree (MODEL.md I-P3). Refused: more than is open, a debt that does not stand,
an account that is no money account, a date before the debt's.

**Bills** lists every bill and every document not booked yet, newest first,
each with its amount, what is open and where it stands; **Pay**, **Upload** for
a document the books lack, **Book it** and **Delete** for one not booked.
CLI: `abo bills [--open] [--json]`, each bill with its id, which `pay`,
`write-off` and `revert` take, and the documents filed and not booked. A
bill's number of the supplier's shows beside its description, its due date
beside its amount, and one with something open past it is overdue, there and
on the page, as an invoice is.

### B5. Write off what will not be paid — Supported (browser, CLI)
What a customer will never pay, or a supplier forgave. Browser: **Write off**
on the Pay page, to an account chosen for it. CLI: `abo write-off <id>
--to <account>`. An invoice is written off to an expense account; a bill to
an income account, or against the expense account it was booked to, which
takes the cost back (one a tax return's turnover box does not count). A
write-off moves no money, so another account is refused (MODEL.md N4).
**Done when** nothing is open, what was open is in that account, and the
debt shows as written off, not settled.

---

## C. Invoicing

### C1. Issue an invoice — Supported (browser, CLI)
**Issue an invoice**: customer (`--to` a line each, name first), one line per item
(`quantity|unit price|description`), optionally date, VAT rate, commodity,
the customer's tax number (printed under their address), why no VAT is
charged, the income account, which payment options it prints, their
reference and a note. CLI: `abo invoice`, with `--customer-tax-id`,
`--vat-reason`, `--income` and `--pay-to <id>`, repeatable. The income
account is `invoice.income` unless said: an export or a reverse-charge sale
may go to an account of its own, `Income:Exports` say, declared first, so
that a box of the tax return reads them apart (`acct:^Income:Exports`); a
credit note takes back what its invoice booked, there. One that is no
income account is refused. With no option chosen it prints them all. With
none it says only when to pay; with one or more, "to the following" or "to
any of the following" and each option's name and details, side by side.
An invoice is stated to the places its commodity is declared with (A2).
Its VAT rate, unless given: none to a customer known to be in another
country — their tax number and the company's each name a country, as EU VAT
numbers do, and the two differ; the configured `invoice.vat_rate`
otherwise. The commodity it is in does not count: it says nothing of where
the customer is. `--vat-rate` (the form's VAT rate) is always the rate. The
invoice, and its preview, say the rate taken and why. At none, a
VAT-registered company says why no VAT is charged, and the refusal says the
rate defaulted and why. The log writes out the rate taken. `--preview <file.pdf>` (the
form's **Preview**) writes the PDF it would have, with the number it would
take, outside the books, PREVIEW across every page, and books, numbers and
commits nothing.
**Done when** the invoice is numbered next in its year (its place among the
year's invoices and credit notes), its PDF is under `invoices/<year>/`, the receivable, income and (under
the effective method) output VAT are booked, and the PDF and entry are in one
commit. VAT follows the configured method: none charged when not registered,
the gross booked to income under a flat scheme. Sending the form twice issues
one invoice. A failure — typst missing, say — leaves no commit, no files and
the number unused. Refused: an invoice at 0% VAT, given or by default, from a
company that is VAT registered, that does not say why; one dated before the invoice numbered
ahead of it (numbers follow dates), or after today (dated ahead by mistake,
it would hold every later one back); and one in another commodity charging VAT
when the books have no price for it in the company currency on or before its
date — the VAT is printed in the company currency too, with the price used.
Refused as well: an undeclared commodity; a VAT rate outside 0 to 100, or any
rate at all when the company is not VAT registered; a line that does not say
what it is for; and a total that is not above nought (to take an invoice back,
a credit note, F5). Issued with a field the books' own
template does not print — one copied in before the field existed — it says
so, naming the field, on the terminal and in the notice.

### C2. Find an issued invoice again — Supported (browser, CLI)
**Invoices** lists every invoice and credit note, newest first: number, date,
customer, total, due date, status, and a **PDF** button — greyed out, saying
why, when the books lack the PDF. Clicking elsewhere in a row opens the
invoice's page. The status is what the amounts say: open, part paid (and what
is open), settled, cancelled, or for a credit note the invoice it credits; an
invoice with something open past its due date is also overdue. **Open** shows
only those with something open. CLI: `abo invoices [--open] [--json]`. An
invoice's page lists what paid it, and what is open.
**Done when** each invoice shows where it stands.

### C3. Invoice in another commodity — Supported
The commodity field (`--currency`), declared with its places; the invoice is
stated to those. It needs a price of its commodity in the company's
currency on or before its date, which it keeps: its VAT is stated in the
company's currency at it, and its entry booked at it (MODEL.md I-E4).
**Done when** the PDF is in that commodity, and the entry has the
receivable in it at its cost in the company's currency, and income and VAT
in the company's currency; a bill owed in another commodity likewise has
its expense in the company's currency, and what is owed at its cost. So
profit and loss is one amount.

---

## D. Reconciling

### D1. Reconcile an account with a statement — Supported (browser, CLI)
**Reconcile**: account, commodity, statement date and the closing balance on
the statement. Tick every posting the statement shows and **Record the
statement**; **Take out the statement** takes out the latest. CLI: `abo
reconcile` to see, `abo record --balance <b> <ENTRY.N…>` (or `--all`), `abo
unrecord`. **Show** updates the figures without changing anything.
`abo reconcile` shows the balance in the books beside what is cleared;
`abo record` says, once it has recorded, whether the books agree, or what is
not yet cleared.
**Done when** the statement is recorded with the postings it clears, as one
commit, and its balance is what they and the earlier statements clear (MODEL.md
I-S3); when every posting to it on or before its date is cleared, the journal
asserts the balance for hledger to check too. Refused: a balance that is not
what is cleared, a posting cleared twice or to another account, a statement in
a closed period. Once it is recorded, nothing is booked to the account in its
commodity on or before its date (N7): dated after it, or the statement taken
out first.

### D2. Reconcile cash or a wallet — Supported (as D1)
Any asset account and any commodity.

---

## E. Reports

### E1. Profit and loss for a period — Supported (browser, CLI)
**Reports**, Profit and loss, a period (`2026`, `2026q1`, `last month`, or
`ytd`, 1 January through today). CLI: `abo pl --period …` (or `-p`), with
`--json` or `--pdf`. The heading names the period's first and last day, as
hledger reads it: `Profit and loss, last quarter (2026-07-01 to 2026-09-30)`;
so does the tax return's.
**Done when** income and expenses are listed with totals and the profit for
the period, matching `hledger balance` over the same period. Downloading a PDF
changes nothing in the books.

### E2. Balance sheet at a date — Supported
**Done when** assets, liabilities and equity are listed in the company's
currency — what is held or owed in another at what it was booked at —
unclosed profit shows as an equity line, and the bottom line (assets less
liabilities and equity) is nil.

### E3. Tax return for a period — Supported once configured
Needs a box for each on the form: `abo config set report.vat.<code> '{ label
= "...", query = "...", invert = true }'`, its figure the balance of the
hledger query over the period, turned over for a box on the credit side.
In the browser: **The tax return** on the company page adds, changes and
removes the boxes, each logged as the `abo config` command that makes it.
Without them the tax return's page links there.
**Done when** each configured box shows its figure for the period, in the
company currency. Amounts in another commodity are valued at the price on the
day of the posting, from the declared prices; with no price the return is
refused, naming the box and the price to add, rather than leaving the amount
out.

### E4. Ask hledger anything — Supported (CLI)
`abo balance …` and `abo register …` pass their arguments to hledger: a query, and the
options that choose what is reported and how — periods, depth, value,
the output format. (A statement clears postings in abo's own tags, not
hledger's cleared mark, so `--cleared` says nothing of reconciling.) Any other option is refused, so that they
read the books and no other file, write none, and take no `hledger.conf`;
neither does any report abo makes.

---

## F. Checking and correcting

### F1. See what was done — Supported (browser)
**Log**: every change to the books, oldest first, in words — "Booked bill 3:
Hosting"; reads and refusals change nothing and are not there. Each opens to
the `abo` command that made it, every default it used written out, and the
diff its commit made, documents shown as binary. The last 50 show, and **Show all** the rest.
CLI: `abo log` (`--since <date>` for a day's changes), and `abo entries
<words>` to find an entry and its id.
**Done when** the last change appears as a command that, typed, would make it
again.

### F2. Undo something just done — Supported (browser, CLI)
Every change that succeeds says so in a green notice that fades after ten
seconds, with **Undo** on it while it is the last change; the **Log** offers
**Undo** on the last change. An Undo button takes back only the change it
was shown with: one made since, in a terminal say, refuses it. CLI: `abo
undo`. The Log shows each change as the command that makes it, quoted as a
shell takes it, with every default it used written out.
**Done when** the books are as they were before the last change, as a new
commit, so git keeps both; an entry taken back keeps its id, which no later
entry is given. Undo again, and the change is made again. Refused when the
last commit is not abo's last change, or files hold changes abo did not
make; and when the last change issued an invoice or a credit note, which may
be with the customer: it is cancelled by a credit note (F5), and the refusal
names the command. Anything older is reverted (F7).

### F8. Changes not made by abo — Supported (CLI, browser)
abo commits every change it makes. A change to the books' files not
committed is someone else's: while there is one, abo changes nothing, `abo
tidy` included, and asks for it to be settled first — committed by whoever
made it (`git commit -m '...' -- <files>`), or taken out (`git checkout --
<files>`) — naming the files, and what the change breaks against the last
commit, for information. It offers no abo command as a way out. Reports
still read. Committed, a change that breaks the books is listed by `abo
check` at its file and line, with how to mend it; changes go ahead on such
books, each saying that problems remain, and refused only for a problem of
their own.
In the browser a page says it in the bookkeeper's words — the books were
changed outside abo, and whoever runs abo settles it — with the files, the
diff and the git commands folded under **For whoever runs abo**. So does an
`abo.toml` that cannot be read, as a page of its own, not a bare error.
**Done when** nothing is changed while files hold changes abo did not make,
and the refusal names both git commands.

### F3. Take out a statement recorded by mistake — Supported (browser, CLI)
**Take out the statement** on Reconcile, or `abo unrecord`: the latest
statement of the account, as only the latest can go (MODEL.md §5).

### F4. Take back a filed document — Supported (browser, CLI)
**Delete** on a document not booked, or a reverted bill's, or `abo unfile
<path>`. One a standing bill has stays; so does one dated in a closed period.

### F7. Revert a booking — Supported (browser, CLI)
For a booking that stands and turns out wrong, the bookkeeping way: open its
page and **Revert**. CLI: `abo revert <id>`, dated today unless `--date` says
otherwise.
**Done when** a new entry carries the opposite amounts and names the one it
reverses, which stays. Refused: one already reversed, one a standing payment
pays (revert that first), an invoice (corrected by a credit note, F5), a
credit note, and one a recorded statement shows (N9): a salary paid from
the bank and on its statement moved as booked, and its opposite would be a
payment the bank never made. Booked in the wrong period, it is corrected
on the other side with `abo post` — the expense moved from one period to
the other — or, if the statement was wrong, the statement is taken out
first.

### F5. Correct an issued invoice — Supported (browser, CLI)
An issued invoice is a document the customer has; it is corrected by a credit
note that cancels it in full, and a new invoice. Browser: open the invoice and
**Correct**, which asks first, saying what the credit note will do, and
**Issue the credit note**: it is issued, dated today, and the invoice form opens
filled in as the cancelled invoice was. CLI: `abo credit <number>` (`--date`,
`--note`), then `abo invoice` as usual.
**Done when** the credit note has the next number in the series, its PDF says
"Credit note" and repeats the invoice's lines negated, and its entry negates
the invoice's. Refused: an invoice a standing payment pays (revert it first),
one cancelled already, and a credit note.

### F6. Close a period — Supported (browser, CLI)
**Closing a period** on the company page, or `abo config set
company.closed_through 2026-06-30`. Nothing dated on or
before it is added, no document there filed or taken out, and no statement
or price there added or taken out; the date only moves forward (MODEL.md N2,
N6). Setting it is a change like any other: while it is the last, Undo takes
it back, for a date typed wrong, as the refusal to move it back says.

---

## G. Phone

### G1. Photograph and book a bill on a phone — Partial
The pages work at phone width, and the upload offers the camera, the photo
library or a file. Each company's pages carry a web app manifest of their own
(DESIGN M6), so each set of books installs as its own app, named for the
company. Chrome finds them installable with no errors, for one company served
alone and for each company of a data directory (`node tests/installable.mjs
<url>`). An iPhone takes the icon from `apple-touch-icon`.
Behind TLS under a real host name it installs too; over plain HTTP there,
Chrome refuses it (`not-from-secure-origin`).
**Open:** not yet added to a real phone's home screen, on Android or on an
iPhone. A phone offers to install only over HTTPS, so try it behind the TLS
that `abo serve` expects in front of it. On the iPhone, see whether an
invoice's PDF or a bill's document, opened from the app, leaves a way back:
they open in the same window, and an app on the home screen has no back
button there.

---

## H. Deferred

Listed so the list is complete; each has its trigger in DESIGN.md.

- **Users** (M5, the rest) — who did what, per person, and who may open which books. More than one set of books is served already (A3).
- **Bank import** (§11) — statement lines matched against booked items.
- **Payroll** (§11) — gross to net, payslips.
- **Year-end closing entries** (§12) — until then unclosed profit is a computed equity line.

---

## Fifth and sixth web runs of 2026-10-07: found and fixed

Agents again worked in the browser alone. The sixth run was clean by the
stop condition in SCENARIOS.md: everyone finished every scenario, no one got
stuck or misread a page, and every refusal said what to do instead. Fixed,
each with a test (tests/regressions.rs):

- Fifth run:
  - A cash count is of the whole box. Its page ticks every movement up to
    the count's date. The row once labelled **To clear** now reads "Per the
    count, less what is cleared". A refused statement or count says to tick
    what it shows.
  - A revert refused for a statement (N9) no longer also advises dating it
    after the statement (N7), since no date makes it allowed. Its advice on
    moving an expense to another period names `Liabilities:Accrued`.
  - **Log** takes a **Since** day, as `abo log --since` does.
  - A forgiven bill is written off to income, or against an expense the
    bill posts to and no other (N4). The Pay page names that expense.
- Sixth run: once a statement is recorded, Reconcile's balance field
  starts empty. It used to keep the figure, and **Show** for the cash box
  then offered the bank's closing balance as what was counted.

Reported and left as decided:
- The VAT default goes by tax numbers alone.
- No confirmation before what undo takes back.
- Nothing is changed while there is a change abo did not make, a mend
  included.
- Log entries are under the git user who made them ("abo demo" for the
  demo books).

## Fourth web run of 2026-10-07: found and fixed

Agents again worked in the browser alone. Fixed, each with a test
(tests/web.rs `fourth_web_run`):

- A report refused because of a change made outside abo now gives, under
  **For whoever runs abo**, the same remedy as the notice: take it out
  (git checkout), or mend it and commit it. It no longer says "abo check
  says how each is mended".
- The companies list flags a company whose books hold problems or a change
  abo did not make, in plain words, with a link to the company.
- Write-off (B5): **To** leaves out `invoice.income` and every account an
  invoice was booked to. Its hint names an account such as Income:Forgiven
  to declare first, with a link to **Accounts and currencies**. N4 is
  unchanged. A bill's or an invoice's page has a **Write off** button next
  to **Pay**.
- A refused posting to an account that a debt is on (I-P3) says where to go
  instead, with links.
- Every list of accounts offered for postings shows only leaves (I-A2): the
  entry form, Pay, Reconcile, and a payment option's account.
- Correct (F5): the new invoice form is filled in as the cancelled invoice
  was given. A VAT rate or income account that came from the default is
  left blank. Its note starts "Replaces invoice N, cancelled by credit
  note M.", which can be edited.
- Words:
  - Books' tiles give accounts in full.
  - The **Short name** hint no longer mentions the command line.
  - Every period hint lists "last quarter" and "this year".
  - Reconcile says what it is for under its heading. Before an account is
    chosen, its amount and date labels fit both a statement and a count.
    When nothing is left to clear, it says "The books agree: ... in the
    books, as counted" (or "as the statement says").

## Third web run of 2026-10-07: found and fixed

Agents again worked in the browser alone. Fixed, each with a test
(tests/web.rs `third_web_run`, tests/regressions.rs):

- Gaps left by the second run. The invoice form can now declare a missing
  currency or account, or add a currency's missing price, and then come
  back as it was left, as the entry and bill forms do. The Pay page needed
  none: it names only accounts that are already declared. A refusal keeps
  "abo check", `abo.toml` and other file names under **For whoever runs
  abo**; above it, the settings are "the company's settings".
- **Revert** takes a date (today unless one is given), as `abo revert
  --date` does.
- A PDF, JPEG, PNG or WebP that was cut short is refused, in the terminal
  too: it must end the way its kind of file ends. The bill form does not
  keep such a document for the next try.
- A VAT-registered company has to give the rate it charges, both on the
  terminal and in the form. A blank rate had charged 0% without saying so.
- Reconcile lists only the accounts money moves through. When a statement
  is taken out, its figures go with it.
- An invoice's page now words its VAT the same way however it is opened:
  the rate, why that rate, and at 0% the reason given. A corrected invoice
  keeps the reason, such as reverse charge.
- Words:
  - A cash box is counted. Reconcile says **Counted**, "the count", and
    "Count of ... recorded", and the advice for a difference covers cash
    over or short.
  - Payment terms have a label.
  - A company that charges no VAT has no tax-return boxes.
  - A bill's description is needed only to book it, not to file it.
  - A bill's page lists its payments and write-offs with their amounts.
  - The Log keeps the diff behind **Show the change to the files**.

## Second web run of 2026-10-07: found and fixed

Agents again worked in the browser alone. Fixed, each with a test
(tests/web.rs `second_web_run`, tests/regressions.rs):

- A change made outside abo, or books that break a rule (F8), showed only on
  Books: every page of the company now says so at the top, says that its
  figures may be wrong, and says whether a report can still be made. A
  report that cannot be made says so in words, not hledger's.
- Forms dropped what was entered. A refused bill keeps its document, so it
  need not be chosen again, and names every problem at once. An account or
  currency not yet declared can be declared from the entry or bill form,
  which comes back as it was left (A2, B1). Reconcile keeps what was ticked.
- Reconcile (D1, F3): the take-out button names the statement's account, and the
  currency too when it is not the company's. Account and Currency are lists.
- Words: notices and the Log say what was done in a sentence, an Undo with
  what it took back; a bill is "bill N" and the supplier's number is "their
  number"; a write-off is not called a payment, and its hints say where
  to declare the account first; each VAT method has a single name, given
  with the word the terminal uses; the VAT hint on an invoice states the
  default in full; a payment option's id is its **Short name**.
- Payment terms can be changed from the company page (A3b).
- A company whose books cannot be read is listed by its name, with a link.
- An account's months: every month of the period is listed, nil ones too.
- Books' tiles group their figures by thousands, as the tables do.

## First web run of 2026-10-07: found and fixed

Agents worked through the workflows in the browser alone. Fixed, each with
a test (tests/web.rs `first_web_run`, tests/regressions.rs):

- Missing against the terminal: closing a period (F6) and the tax return's
  boxes (E3) from the company page; filing a bill to book later (B2); an
  account's page by period, with each month's total.
- Said in the computer's words: refusals named rule codes and files, and
  the hint the terminal gives was missing; a change made outside abo, or
  problems in the books, showed git's text; an unreadable `abo.toml` a bare
  error. Now said in words, with what is technical folded under **For
  whoever runs abo**, and the hint chosen by the rule broken and the field
  it is about, never by its wording.
- Notices named commit hashes and doubled their verbs; the Log was newest
  first in commands, now oldest first in words (F1).
- **Correct** issued a credit note at one click; it asks first (F5).
- **File and book it** was two changes, and one Undo took back half (B2).
- The same supplier number, said to be another bill, was booked: it is
  that bill whatever is said (B3, N8; in the terminal too).
- The issued invoice did not say which VAT rate it took and why; reports
  took a bad period as an error page, now said on the page; Books valued
  holdings at a price, not at cost; the last booked showed by id, not
  date; PDFs downloaded without their number as their name; Reconcile's
  **Show** without an action was an error.
- Words: "Declarations" is **Accounts and currencies**; the company is in
  the sidebar; the VAT methods say what each means; the new-company form
  asks who keeps the books in plain words, folded, without git's.

## Short ninth CLI run of 2026-10-07: the two changes read as meant

Two agents, setting up and invoicing, to try the VAT default by tax numbers
and the hand edit's refusal. Both read as meant: each invoice said its rate
and why, and the refusal's diff and reason took the agent straight to `git
checkout`. Fixed, with a test: reports on books holding a change abo did
not make warned only of "a problem", where they now say what it is; the
derived-line message offered `abo tidy`, which a change not committed
holds up; `abo price` with none printed nothing.

## After the eighth run: the VAT default by tax numbers alone

The default VAT rate went by currency as well as tax numbers: a Polish
customer billed in USD got none, a Swiss one billed in EUR the full rate,
the same customer two ways by currency alone. Now tax numbers alone decide
(none when both name countries and they differ, the configured rate
otherwise), and every invoice and preview says the rate it took and why.

## After the eighth run: a hand edit's refusal says what and why

What the agents ranked first in the eighth run was a rule kept: while the
books' files hold a change abo did not make, abo runs no command, not even
the one that would mend what the change broke. Kept, strict; its messages
were not good enough, and now are: the refusal shows the change itself,
line by line, what it breaks, why abo runs nothing while it is there (its
commit would take the change in as abo's own), and the two ways out; `abo
check` says the same first; `abo --help` and `abo check --help` state the
rule.

## Eighth CLI run of 2026-10-07: clean but for decisions taken

Every scenario done; every refusal met said what to do next; every hand
edit caught, with its file and line; output held to ASCII. What the agents
ranked first was what was decided: `abo account add` refused while a change
abo did not make is there (F8), and the VAT default by currency where no
tax number names a country. Fixed, each with a test under "the eighth CLI
run": `abo vat` needs its period, a return for all time never being filed;
the warning that the rate took no account of where the customer is showed
with a rate given; `bills`, `invoices` and `entries` did not warn of
problems the books hold; a bill's number was cut off with its description,
and now has a column of its own; `abo bill --help`'s example named an
account fresh books lack.

## Seventh CLI run of 2026-10-07: found and fixed

Every scenario done; no agent stuck for more than about two minutes; every
refusal met said what to do next; output held to ASCII. Fixed, each with a
test under "the seventh CLI run": `bal` and `reg` refused `-p ytd`, which
`pl` and `vat` take; `abo bill --help` read as though abo wrote the debt's
posting, and its refusal gave no posting to add; reports on broken books
did not point to `abo check`; the hint for splitting an account showed on a
booking; an invoice named its VAT rate only in its PDF, and a customer of a
country, invoiced by a company whose tax number names none, got the
company's rate unsaid; `record` did not repeat what the statement says.

Open, as before: a bill has no number of the supplier's and no due date.
Not abo's to fix: the demo's tax number names no country, so its VAT
default cannot tell a customer abroad; a VAT return without a box for the
VAT payable is the configuration's.

## Sixth CLI run of 2026-10-07: found and fixed

Every scenario was done; no agent was stuck for more than about a minute.
Found and fixed, each with a test under "the sixth CLI run": a hand edit of
an invoice's derived postings, committed, was written back inside the next
booking's commit, unsaid — text not as abo writes it now holds every change
up until `abo tidy`, and `check` says the line is derived; a document named
by the path of its own file was "not filed"; plain `abo bal` came out short
of nil, what is owed in USD not at its cost — `bal` and `reg` are at cost
unless told; the N9 hint named an account the books lacked; `undo` still
offered to commit what breaks the books; splitting an account in use said
no way round; a crypto option's wallet was `--address`, now `--wallet`.

Open, for a decision: a bill has no number of the supplier's and no due
date.

## Fourth and fifth CLI runs of 2026-10-07: found and fixed

The fourth run, after the decisions of the third (ids given once, no undo of
an issued invoice, dated headings, defaults shown, N8 within a week, N9,
`ytd`, `--income`), was stuck nowhere but over a hand edit; its findings
were decided (F8, VAT abroad, a bill written back to its expense, preview,
a VAT company's empty return) and made. The fifth, on those, found: an
entry booked on top of one that does not balance, once that was committed,
went ahead with a warning — now nothing is added to books that break a rule
but what mends one (MODEL.md §8); a change not committed that breaks the
books was answered "commit them yourself" first — now only taken out, or
mended first; `TAX-123` read as a tax number of country TA, so a customer
with any tax number went without VAT — now only a country's code followed
by a digit (or `ATU`, `CHE`) names one; reconcile, record and unrecord took
`Assets:Bank` whatever the bank's option pays into; a refused removal said
to declare the account; an unbalanced entry did not say by how much; `ytd`
headed a report as "ytd"; an entry without an id was "the tags [] say no
kind of entry". Each has a test under "the fifth run".

## Third CLI run of 2026-10-07: found and fixed

Not clean, though no agent was stuck for long but over a hand edit, and
what remains is mostly for a decision rather than a fix. Fixed, each with a
test under "the second CLI run": `abo check`, for a hand edit not committed,
still gave the abo command that the edit then held up, and the refusal's
"commit them" led to committing broken books. The advice now follows from
what the edit breaks: if nothing, `abo tidy` takes it in; if something, it
is taken out with `git checkout`, or mended by hand. Also: the files an
unreadable file's message names, `today` as a date, the help of
`payment-options edit` and `remove`, an option's account declared with it as
its help now says, N8 writing an amount to its places.

Open, for a decision: a bill booked twice a few days apart is not caught
(N8 is the same day); an entry's id is taken again after an undo; an issued
invoice taken back by undo gives its number up again; a reversal dated today
of what moved money leaves the bank with a movement no statement will show;
a period's heading names no dates; reverse-charge sales share an income
account with the rest.

## Second CLI run of 2026-10-07: found and fixed

The same four scenarios' agents, afresh, on the fixes of the first. Not
clean: three of four went round in a loop over a hand edit. Each fault has
a test under "the second CLI run, 2026-10-07". Output held to ASCII.

| Workflow | Found, fixed |
|---|---|
| (S26) | `abo check` said to declare an account; the hand edit not committed then refused that, naming no way out. The refusal now gives the git commands that commit or take out the edit. A change made on books holding a problem went ahead silently; it now warns that they hold problems. `tidy` wrote back a hand edit of an invoice's derived postings and said "Nothing to tidy"; it now says what it wrote back, and why. An unreadable file now says how to mend it |
| B3 | The same bill split otherwise among expenses got past N8, which now compares what is owed, not every posting |
| E2 | The balance sheet showed what is owed in USD in USD, beside hledger's conversion accounts; it is now at cost, in the company's currency |
| B5 | A debt written off showed as settled; now "written off" |
| B4, F2, D1 | pay, write-off and revert said "Booked entry N" and not the amount; undo said only "Took back the last change"; unrecord named no statement. Each now says what it did |
| E1 | A loss was "Profit for the period" below nought; `pl` and `vat` took no `-p` |
| A3 | A command run in a data directory named none of the companies there |
| B3 | Filing a document twice did not say which bill it is the document of |

Not abo's: messages init printed after the next question came from the run's
own wrapper, which showed what abo wrote to stdout before what it wrote to
stderr. Left as they are: a period's heading says `last quarter` without its
dates; entry ids follow from their place (see below); a reverse-charge sale
is booked to the same income account as any other.

## CLI run of 2026-10-07: found and fixed

The first run of SCENARIOS.md, the command line alone: four fresh agents
with a shell, the books and no source, judged on the books afterwards. Not
clean: each got stuck somewhere. Each fault has a test in
`tests/regressions.rs`, under "the CLI run of 2026-10-07". Its output held
to ASCII throughout.

| Workflow | Found, fixed |
|---|---|
| C1, C3, E1, E2 | An invoice in USD was booked in USD, so profit and loss and the balance sheet came out in two currencies while the tax return converted. Income and expenses are now in the company's currency (I-E4), an invoice in another commodity always priced, and booked at that price |
| D1, F7 | A reversal dated back to May went under two recorded statements and took the assertion off the first; nothing is now booked on or before a statement of its account (N7) |
| B3 | The same bill, without its document, was booked twice without a word; now only with `--again` (N8) |
| F6 | The refusal to move a closing date back said the books "stay closed", where undo reopens them while it is the last change; it now says so |
| (S26) | Hand edits: `abo tidy` committed an entry that did not balance and called it tidied; declaring an account in use was refused for the very problem it mended; a report blamed its period, or showed hledger's command line; `abo check` named no line, abo.toml's error no line; a canonical hand edit was "nothing to tidy" while every change stayed refused for it |
| B2, B4 | No CLI list of bills, their ids and what is open, nor of documents filed and not booked: `abo bills` |
| F1, F7 | No command showed an entry's id: `abo entries`; no day to start the log from: `abo log --since` |
| A1 | Without a terminal, init printed only its questions of several lines |
| A4 | A crypto option's default account was refused as undeclared; an option's account is now declared with it. `--detail` is `--details`, as abo.toml has it, and init's `--payment` takes a list of lines |
| C1 | `--to` took `\n` within one argument; now a line each, as `--address` |
| D2 | `abo reconcile` never showed the book balance |
| (all) | Refusals said what was wrong and not what to type: they now end with the command or flag that puts it right; paying a settled bill said "the amount must be above nought". Help: closing a period, write-off's account, price, record and unrecord's arguments, the options bal and reg pass on, a report's default period |

Left as they are: an entry's id is its place among the entries, so an
undone entry's id is the next one's (MODEL.md §2.1: only appended, undo
removes the last); `reg --json` is hledger's JSON, not abo's.

## Verified: 2026-10-04

A fifth run, after the CLI and the web moved onto the model: four fresh agents,
working from this file as before. Each fault found has a test in
`tests/regressions.rs`, under "the workflow run of 2026-10-04".

| Workflow | Fifth run |
|---|---|
| A3b, B1, B3, B4, B5, C2, C3, D1, D2, E1, E3, F3, F5, F7 | Verified |
| A1, F1, F2 | Failed, fixed: the Log showed a command unquoted, and one typed in a terminal without the defaults it took, so neither typed back to the same change; it showed `bills/index` as binary; an Undo button left on screen took back a change made since in a terminal |
| A2, A3, A4, F6 | Failed, fixed: a refused removal said "is not declared"; `abo demo --dir` was ignored, every demo sharing one directory; the help said an added option prints last. Written down: an unchanged option changes nothing, as the company form; a comment in `abo.toml` is a hand edit; Undo takes back a closing date while it is the last change |
| B2, F4 | Failed, fixed: `abo bill` crashed on a document not filed, and took a date other than its document's; `abo file` filed into a closed period; a document uploaded later was named after the description; a reverted bill's document was offered for Delete, and Delete refused it |
| C1, G1 | Failed, fixed: a VAT rate so small its VAT rounds to nought got past the reason 0% needs; I-I1 said only "not one abo could have issued"; a stray brace left most of the stylesheet to phone width alone, so notices never faded on a desktop |
| E2, E4 | Failed, fixed: an exchange booked at cost left the balance sheet short of nil; a VAT box's query could be an hledger option writing a file; `--pdf` could write over a filed document |

## Verified: 2026-10-01

Run four times that day, by four fresh agents each time, each with its own copy of
the demo books and its own server, typst installed, working from this file
and not the source: setting up (A, F6); booking, reconciling and reports
(B1–B3, D, E); invoicing and corrections (C1, C3, F5, F7, G1); bills,
settling, the Log and undo (B4–B6, C2, F1–F4). Every fault of the first run
held fixed in the second, and of the 40-odd fixed after the second all but
three held in the third, those three having been fixed too narrowly. What the
third found was fixed the same day, with a test for each in
`tests/regressions.rs`, under rules 3, and not yet verified again.

The fourth run found that fixing each fault where it showed had stopped
converging: tags typed by hand got past every check made for one tag alone,
undo was checked for bills but not invoices, invisible characters slipped a
list kept by hand. Under rules 4, abo's own tags are reserved to the commands
that write them, a payment pays one bill or invoice, an undo may leave no new
problem anywhere in the books, characters are refused by their Unicode
category, and hledger reads no config and takes only listed options.

| Workflow | Fourth run |
|---|---|
| A2, B1–B3, C3, E1–E3, F1, F3, F6, F7 | Verified |
| B4, B5, B6, C2, F2, F4 | Failed, fixed: a hand-typed `reverses:`, `credits:` or `invoice:` was taken — reversing twice, cancelling any invoice, duplicating a number; a `?` bill could be settled for 0.01, and an invoice overpaid or paid in another commodity; an undo could settle an invoice twice, bring back a credit note without its invoice, or leave a reversal taking back what is not there; an undo of an undo, an entry booked above it since, failed with hledger's error |
| A1, A3, A3b, A4 | Failed, fixed: flags to init, and a label, were not trimmed; saving an option unchanged in the browser added the company as holder; more invisible characters slipped through; a data directory served `-lead` and listed `Odd_Name`; `abo.toml` could hold an IBAN, id, name or VAT account no command would take; a value of `ABO_RULES` naming no rules said nothing, and pages never said older rules applied |
| D1, D2, E4 | Failed, fixed: a recorded balance was still written as typed; showing figures took dates recording refused; `abo bal` read other files, by `@args`, `--conf` or a `hledger.conf` above the books, and wrote them with `-o` |
| C1, G1 | Failed, fixed: a huge due period crashed abo; numbering stopped at 9999; the Log could not be read at phone width, and a bill page with a long path scrolled sideways |

| Workflow | Third run |
|---|---|
| A2, A3, B3, B5, C1, C2, C3, D1, D2, E1–E4, F1, F3, F4, F6, F7 | Verified |
| B4, B6, F2 | Failed, fixed: an undo or a revert could leave a bill paid more than it owes, or shown settled with money still owed, while Bills showed nothing open; a `takes-back:` typed by hand counted as a reversal's, so a bill could be cancelled under its payment and an invoice settled twice; a part paying all a `?` bill owed was taken |
| B1, B2 | Failed, fixed: `abo file` and `abo invoice` compared a date as text before reading it — `2026-1-5` was filed in a closed period, `26-9-1` as the year 0026; the browser's File and book filed the document and then refused the booking, over a hand edit in the year file; `100.000 EUR` was written and reported as typed, and a recorded balance too |
| F5 | Failed, fixed: Correct forgot where the invoice asked to be paid and the places it was stated to; the form now has Places |
| G1 | Failed, fixed: at phone width the invoice list hid each invoice's date and due date |
| A1, A3b, A4 | Failed, fixed: an answer of spaces was taken from flags; a bad line in an address hid every other problem in the form; an edit answered as it stood still changed the holder; `abo.toml` could send money to an account that is no asset; `credit --note` and a few rare invisible characters slipped through |
| (in passing) | Fixed: Pay pages took money in another commodity than the bill's or invoice's; a negative due period, and a reversal dated after today, were taken; an undo held up by a hand edit blamed "something since"; a company form missing a field cleared it; `abo bal -f` read another file. The tests' servers now bind a port the system picks, and each run clears away the books of runs gone before |

| Workflow | Second run |
|---|---|
| A1, A2, A3, B1–B3, B5, C1, C2, C3, E1–E4, F1, F3, F5, F6, F7, G1 | Verified |
| A3b, A4 | Failed, fixed: `abo company` and `payment-options add` with flags took control characters; a cash option without lines was taken; money could arrive in an expense or income account. Also: init took VAT accounts of the wrong kind |
| B4, F2 | Failed, fixed: an undo could leave a bill part paid and cancelled, or paid and not booked; a payment of a bill whose open amount is `?` could be any amount; a `pays:` of all that was open left a bill that could never be settled |
| B6 | Failed, fixed: an invoice could be settled or paid before its own date |
| D1, D2 | Failed, fixed: a statement recorded could be recorded again, unasserted, at another balance; the browser's clear took an undeclared or other commodity, a balance that is no number, and any action |
| F4 | Failed, fixed: the undo of an undone Upload was refused, for the payment the upload had moved |
| (found writing a test) | Fixed: an undo checked the books before putting back a year file's `include`, so an entry it put back in another year went unchecked — a payment of a bill no longer booked among them |
| (thought of, then tested) | Fixed: a link or key made before another entry of the same day was booked above it reached that other entry — keys now carry a fingerprint of their entry; a commit swept up whatever else was staged; an invoice could be dated ahead, holding every later one back; an invoice below the last numbered could be undone, leaving a gap; zero-width and right-to-left characters were taken in text; one company's form token was taken as sent in another's; `abo settle --by` took a reversed payment, or one dated before the invoice; a document could be filed for a booking in a closed period, rewriting its tags; an asset declared in lower case was not offered to pay from; a description could make the Log take a commit for undone; a key could reach a twin entry that differed only in its tags; one inconsistency made by hand refused every later undo; an `abo` at a terminal and the server could write the books at once, colliding on git's index — the books are now held for one change at a time; a clearing could be undone under a statement recorded since; `abo.toml` edited by hand was not checked (a payment id twice, a receivable that is no asset, an account not declared); abo wrote into a year file holding someone else's uncommitted change; a log taken under laxer rules could not be replayed — each line now names its rules, and a replay takes it under them, and abo says so whenever it checks by older ones; a page read while a terminal's abo was mid-change could show what was never booked — it now waits for the change; line and paragraph separators and a few other invisible characters were still taken; a stale link's page did not say what to do |
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
