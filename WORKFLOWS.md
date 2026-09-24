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
`abo init` in an empty directory asks for the company's name, address, country,
tax number, payment details, currency, VAT method and the rate charged to
customers.
**Done when** the directory is a git repository with one commit holding
`abo.toml`, `journal/main.journal`, `journal/accounts.journal` (the default
accounts, plus the VAT accounts the method needs) and `.gitignore`.

### A2. Declare an account or a commodity — Supported (editor, by design)
Add an `account …` or `commodity …` line to `journal/accounts.journal` and
commit it. There is deliberately no editor for this in abo (DESIGN §12).
**Done when** the new account can be posted to; an undeclared one is refused
with a message saying where to declare it.

### A3. Start the web interface — Supported
`abo serve` for the books in the current directory, `abo demo` for throwaway
toy books. A missing `git` or `hledger` stops startup with a message naming
what it is for; a missing `typst` is a warning.
**Done when** the front page shows the company, its balances and recent entries.

---

## B. Everyday booking

### B1. Book an entry by hand — Supported (browser, CLI)
Browser: **Entry**, fill in date, description and one posting per line
(`account=amount`, one amount may be left off). CLI: `abo add`.
**Done when** the entry is in the year's journal file in date order, one commit
holds it, and the front page says **Booked** with that commit. An entry that
does not balance, names an undeclared account, posts to a VAT account the
method forbids, or falls in a closed period is refused with the reason, and the
form comes back filled in.

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
committed before approval.

### B4. Come back to a bill later — Supported (browser, CLI)
A proposal not yet approved waits. The front page lists it under
**waiting for you** and Bills shows a count; uploading the same document again,
or re-running `abo bill` on it, resumes the same conversation.
**Done when** the resumed proposal is the one left, with its corrections, and no
second request was paid for.

### B5. The same bill twice — Supported
Uploading a document already booked is refused before any request to the model.
**Done when** the refusal names the entry that already holds it.

### B6. Record a payment or a receipt — Supported (as B1)
Paying a bill, a customer paying an invoice, moving cash: an ordinary entry
(B1) between the bank or cash account and payables or receivables.
**Done when** the payable or receivable it settles goes to zero.

---

## C. Invoicing

### C1. Issue an invoice — Supported (browser, CLI); needs typst
**Issue an invoice**: customer, one line per item
(`quantity|unit price|description`), optionally date, VAT rate, commodity,
their reference and a note. CLI: `abo invoice`.
**Done when** the invoice is numbered one past the highest `invoice:` tag for
its year, its PDF is under `invoices/<year>/`, the receivable, income and (under
the effective method) output VAT are booked, and the PDF and entry are in one
commit. VAT follows the configured method: none charged when not registered,
the gross booked to income under a flat scheme.

### C2. Find an issued invoice again — Partial
`/invoice/<number>` serves its PDF, and the browser opens it right after
issuing. **Gap:** nothing in the interface lists issued invoices; today they
are found through recent entries, or in `invoices/<year>/`.

### C3. Invoice in another commodity — Supported; needs typst
The commodity field (`--currency`) and `[invoice] decimals` (`--decimals`).
**Done when** the entry and the PDF are in that commodity with its places.

---

## D. Reconciling

### D1. Reconcile an account with a statement — Supported (browser, CLI)
**Reconcile**: account, commodity, statement date and the closing balance on
the statement. Tick every outstanding item the statement shows and
**Mark ticked as cleared**; when the difference is nil, **Record this balance**.
CLI: `abo reconcile`, `abo clear`, `abo reconcile --record`.
**Done when** the ticked transactions carry `*` in the journal, each marking is
a commit, and recording writes a dated entry — with a balance assertion when
nothing is outstanding, without one otherwise.

### D2. Reconcile cash or a wallet — Supported (as D1)
Any asset account and any commodity.

---

## E. Reports

### E1. Profit and loss for a period — Supported (browser, CLI); PDF needs typst
**Statements**, Profit and loss, a period (`2026`, `2026q1`, `last month`).
CLI: `abo pl --period …`, with `--json` or `--pdf`.
**Done when** income and expenses are listed with totals and the profit for
the period, matching `hledger balance` over the same period.

### E2. Balance sheet at a date — Supported; PDF needs typst
**Done when** assets, liabilities and equity are listed, unclosed profit shows
as an equity line, and the bottom line (assets less liabilities and equity) is
nil.

### E3. Tax return for a period — Supported once configured; PDF needs typst
Needs `[[report.vat]]` lines in `abo.toml` (DESIGN §6). Without them the page
says what to write.
**Done when** each configured box shows its figure for the period.

### E4. Ask hledger anything — Supported (CLI)
`abo bal …` and `abo reg …` pass their arguments to hledger.

---

## F. Checking and correcting

### F1. See what was done — Supported (browser)
**Log**: every operation asked for, newest first, including refusals. Each
opens to its input and, for a commit, the diff it made.
**Done when** the last action appears with the right input and outcome.

### F2. Undo a booking made by mistake — Missing
DESIGN §1 names `git revert` as the undo, and that works from a terminal today.
**Open decision:** whether undo in abo is a revert (the entry disappears from
the journal; git keeps the history) or a reversing entry (the entry stays and a
mirror entry cancels it, the usual bookkeeping practice), and whether the
browser offers it from the Log or from recent entries.

### F3. Un-tick a transaction cleared by mistake — Missing
Clearing writes `*`; nothing removes it except editing the journal.
**Open decision:** an **Unclear** action beside cleared items, or undo (F2)
covering it.

### F4. Throw away a pending bill — Missing
A document uploaded by mistake, or not a bill at all, stays waiting in
`.abo/pending` for ever.
**Open decision:** none really; it needs a **Discard** button and `abo bill
--discard`.

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
