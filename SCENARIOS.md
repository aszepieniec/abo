# Scenarios

What a person sets out to do with abo, in their words and with their figures,
naming no interface. The same scenarios are worked through the CLI and through
the browser; each is judged by what the books hold afterwards, against
**Done when** of the workflow in WORKFLOWS.md it names. The CLI and the browser
are two ways into the same books: a scenario that can be finished in one and
not in the other is a fault.

The person working a scenario reads this file's scenarios and its brief, and
nothing else of the repository: not WORKFLOWS.md, not DESIGN.md, not the
source. What they learn of abo, they learn from abo.

---

## Run

Four people, each on books of their own:

| Person | Starts from | Scenarios |
|---|---|---|
| 1 | an empty data directory | S1–S5 |
| 2 | a copy of the demo books | S6–S11 |
| 3 | a copy of the demo books | S12–S17 |
| 4 | a copy of the demo books | S18–S25 |
| 5 | a copy of the demo books | S27–S31 |

Each then does S26 on their own books, last.

The demo books are those of Example Ltd: EUR, VAT by the effective method at
10%, one bank payment option. Their dates count back from the day they were
made: an opening balance about nine months ago, bills from Acme Hosting AG,
Lex & Partners (part paid) and Devtools Inc (in USD, unpaid), two invoices
(one settled, one part paid), a bill document from Telecom GmbH filed and not
booked, and a bank statement recorded a week ago. Acme Hosting AG, the two
customers, Nordlicht AG and an employee, Sam Example, are partners; Nordlicht
AG has a monthly retainer invoiced at the start of each month, this month's
not yet, and Sam a salary paid after each month, last month's not yet.

A run counts as clean when no person got stuck, misread what abo said, met a
refusal they could not act on, or saw a character outside printable ASCII on
the CLI. Faults are fixed, each with a test in `tests/regressions.rs`, and the
run is repeated until one is clean.

---

## 1. Setting up

**S1. A new company** (A1). Open the books for *Kowalski & Daughters Sp. z
o.o.*, ul. Długa 5, 00-238 Warszawa, Poland, tax number PL5260001246. They
invoice in PLN, are VAT registered, account for VAT as charged and paid, and
charge 23%. Customers pay into their account at PKO BP, IBAN PL61 1090 1014
0000 0712 1981 2874, BIC BPKOPLPW.

**S2. A second company in the same place** (A1, A3). Open the books for *Anna
Weber*, a sole trader in Zurich, Switzerland, not VAT registered, invoicing in
CHF, paid in cash at the office on Fridays. Both companies' books must be
there, apart, afterwards.

**S3. They moved** (A3b, A4). Kowalski & Daughters moved to ul. Złota 44,
00-120 Warszawa. They also take payment in bitcoin now, on the Bitcoin
network, to bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh, received into an
account of its own. The next invoice must say both.

**S4. Accounts of their own** (A2). Kowalski & Daughters want software
subscriptions apart from other expenses, and a savings account at the bank
apart from the current one. They also have customers paying in USD (two
decimal places); one US dollar was worth 3.62 PLN on the 1st of this month.

**S5. The first half year is done** (F6). The books of Kowalski & Daughters
are closed through 30 June 2026. Then try to book a 20 PLN bank fee dated 15
June 2026: it must not get in, and you must be able to tell why.

---

## 2. Bills and money out

**S6. Rent** (B1). Rent of 1,800.00 EUR for this month was paid from the bank
on the 1st. There was no bill.

**S7. A bill on paper** (B2). The scan `bill-scan.pdf` in your home directory is
a bill from Officeworld GmbH, dated the 3rd of this month: office chairs,
400.00 EUR plus 40.00 EUR VAT, to be paid later. Put it in the books, document
and all.

**S8. The same bill again** (B3). A colleague, not knowing, tries to put the
same scan in the books again. It must not get in twice.

**S9. The waiting document** (B2). There is a phone bill from Telecom GmbH
that was filed and never booked: 59.00 EUR, no VAT. Book it.

**S10. Paying what is owed** (B4). Pay Lex & Partners what is still open on
their bill, from the bank, today. Pay half of Officeworld's bill today, and
see that the rest is still shown as owed.

**S11. Forgiven, and filed by mistake** (B5, F4). Devtools Inc say the
licence is free for this year: their bill will never be paid. Separately, you
filed a document by mistake (file any PDF of your own making first): take it
out again.

---

## 3. Invoicing and money in

**S12. An invoice** (C1). Invoice *Nordlicht AG*, Seestrasse 12, 8002 Zürich,
for 12 hours of consulting at 120.00 EUR and one workshop at 900.00 EUR, dated
today, to be paid by bank transfer. Look at the PDF.

**S13. No VAT, and why** (C1). Invoice *Brightside Ltd*, a customer in Ireland
with VAT number IE6388047V, for 2,000.00 EUR of consulting. The VAT is
reverse charged: none is charged, and the invoice must say why.

**S14. In dollars** (C3). Invoice *Hudson Partners LLC*, New York, for 1,500.00
USD of consulting, dated today. The books need to know what a dollar is worth
in EUR; take 0.92.

**S15. Who still owes** (C2). Which invoices are not fully paid, how much is
open on each, and which of them are past their due date?

**S16. Money comes in** (B4). Nordlicht AG pays the S12 invoice in full into
the bank today. Customer Three pays what is still open on theirs.

**S17. A wrong invoice** (F5). The Brightside invoice should have been 2,200.00
EUR. The customer has the first one already; correct it the way an
accountant would.

---

## 4. Reconciling, reports, correcting

**S18. The bank statement** (D1). Before you start, book two bank movements
dated two days ago: a 12.00 EUR bank fee and a 300.00 EUR transfer to petty
cash. The bank statement dated yesterday shows every bank movement booked up
to then except the transfer, which the bank has not processed yet. Record the
statement, with the closing balance it must then show.

**S19. Counting the cash** (D2). The cash box held 757.50 EUR when counted
yesterday evening. Record it, and say whether the books agree.

**S20. The wrong statement** (F3). It turns out the S19 count was from the
wrong box. Take it out.

**S21. Profit and loss** (E1). What was the profit for the year to date? For
last quarter? Get the year's as a PDF for the owner.

**S22. Balance sheet** (E2). What does the company own and owe as at today,
and does it balance?

**S23. The VAT return** (E3). What goes in each box of the VAT return for last
quarter?

**S24. A question for the books** (E4). How much went on hosting this year,
month by month?

**S25. Taking things back** (F1, F2, F7). Book a 50.00 EUR taxi fare paid in
cash, then take it back at once: it was a mistake. Then find the earliest
salary paid to Sam Example and reverse it: it was booked in the wrong
period; it belongs to April.
Afterwards, list what was done to the books today, in order.

---

## 5. When the files are changed by hand

**S26. A hand edit** (none). The books are plain files and you may edit them;
when you do, abo promises nothing. Make three edits, one at a time, each
followed by some ordinary work with abo (a report, a booking), and each put
right before the next:

1. an entry in the journal whose amounts do not balance;
2. an account that entries post to, taken out of the declarations;
3. `abo.toml` made unreadable — a missing quote, say.

For each: did abo notice? Did it say which file and line? Did it say how to
put it right? None of these is needed to pass; each is recorded.

---

## 6. Partners and agreements

**S27. A new supplier** (A5, B7). Officeworld GmbH, Industriestrasse 3, 80331
München, Germany, VAT number DE811234567, now supplies the office. Keep them
as a partner, and book their bill no. OW-1042 of the 3rd of this month: paper,
80.00 EUR plus 8.00 EUR VAT, as theirs. Then try to book the same bill no.
OW-1042 again, for 88.00 EUR in all: it must not get in.

**S28. Cleaning** (A6, B7). From the 1st of last month, Clean & Co (Gasse 7,
8001 Zürich) clean the office for 250.00 EUR a month, paid from the bank
after each month, without a bill. Keep the contract, and pay last month's
cleaning. Pay what Sam Example is owed for last month too: their salary is
as before.

**S29. The retainer** (C4). Invoice Nordlicht AG this month's retainer.
Nordlicht's retainer goes up: from the 1st of next month, 1,800.00 EUR a
month. Afterwards, the retainer must show what was invoiced under which
terms, and what next month's invoice will say.

**S30. Signed, and ended** (A6). File `contract.pdf` from your home
directory as Nordlicht's retainer contract, as signed. Sam Example leaves:
their last day is the last day of this month.

**S31. The same again** (C5). Customer One wants the consulting of their
invoice again, at the same price, invoiced today.

---

## Brief: CLI

Given to each person, with their scenarios.

> You keep the books of a small company, and you use **abo**, an accounting
> program you have not used before. You have a shell, with `abo`, `hledger`,
> `git` and the usual Unix tools on the PATH, and a data directory at
> `$BOOKS`. That and your home directory are all you may read or change. Do
> not look for abo's source or documentation elsewhere on the machine, and do
> not use `abo serve` or `abo demo`: this run is about the command line.
>
> Learn abo as a new user would: from `abo --help`, `abo <command> --help`,
> and what it says back. You may read and edit the files in `$BOOKS`, but
> once you edit one by hand, abo promises nothing about it; if you edit one
> because abo would not do what you needed, that is itself a finding — say
> what you needed.
>
> Your shell is not a terminal: where abo asks questions, answer them on
> standard input, or with the command's flags.
>
> Work through your scenarios in order. For each, report:
>
> - **Outcome**: done, done with trouble, or not done.
> - **What you typed**: every `abo` command, in order, the failed ones too.
> - **Findings**, each with the command and abo's output quoted verbatim:
>   - where you got stuck, and how long it took to get unstuck;
>   - output you misread at first, and what you took it to mean;
>   - a refusal whose message did not tell you what to do instead;
>   - help that was wrong, missing, or disagreed with what the command did;
>   - two commands that call the same thing by different names, or take
>     it in different ways;
>   - any character in abo's output outside printable ASCII;
>   - anything abo let you do that a bookkeeper would think wrong.
>
> Last, say which finding you would want fixed first, and why.

## Brief: web

Given to each person, with their scenarios. S26 is not theirs to do: the
person running the run makes each hand edit in their books, at the point
S26 names, and tells them only that "someone changed the books outside abo";
they then find out what the pages say, and whether they can put it right.

> You keep the books of a small company, and you use **abo**, an accounting
> program you have not used before, in a web browser. abo is running at
> `$URL`. You have a browser and nothing else: no shell, no files but those
> named in your scenarios, which you may upload. You drive the browser with
> the `web` tool, one action a call:
>
> - `web go <url>` opens a page; `web back` goes back.
> - `web read` shows the page as a person sees it: its title, its text, and
>   every link, field and button, each numbered (`L3`, `F2`, `B1`), with a
>   field's label, its value and, for a list, its choices.
> - `web fill F2 'text'`, `web choose F4 'choice'`, `web tick F5`,
>   `web upload F6 <file>` fill a field; `web click B1` or `web click L3`
>   presses a button or follows a link, and shows the page it leads to.
> - `web shot <file.png>` takes a picture of the page, which you may look at
>   to see what `web read` cannot show: layout, colour, what stands out.
>
> Learn abo as a new user would: from its pages, what they say, and what
> they say back. Do not look for abo's source, its documentation or its
> files, and do not type addresses abo's pages do not lead to, but the one
> you start at.
>
> Work through your scenarios in order. For each, report:
>
> - **Outcome**: done, done with trouble, or not done.
> - **What you did**: every page, field and button, in order, the failed
>   attempts too.
> - **Findings**, each with the page and its words quoted verbatim:
>   - where you got stuck, and how long it took to get unstuck;
>   - what you misread at first, and what you took it to mean;
>   - a refusal whose message did not tell you what to do instead;
>   - a page that did not say what it is for, or what a field wants;
>   - two pages that call the same thing by different names, or take it in
>     different ways;
>   - anything you could not find, or found only by guessing;
>   - anything abo let you do that a bookkeeper would think wrong.
>
> Last, say which finding you would want fixed first, and why.

## Setting up a web run

For each person: a fresh copy of the books (the demo's, or an empty data
directory for person 1), and `abo serve --dir <it> --port <their own>`,
its output logged outside their reach. A headless Chrome of their own,
started with `--remote-debugging-port`, and the `web` tool on their PATH:
a short Python script that connects to that Chrome by Playwright over the
DevTools protocol, does one action on its one tab, prints the page as
`web read` does, and logs the action, the address and the answer outside
their reach. Their shell is used only to call `web`; the brief forbids the
rest, as the CLI brief forbade abo's source.

The judging is as for the CLI: each scenario's books, journal and `git log`
against WORKFLOWS.md. The web may use typography; the ASCII rule is the
CLI's alone.

## Setting up a run

For each person: a fresh directory under the scratchpad as `$HOME`, holding
`bill-scan.pdf` (person 2) or `contract.pdf` (person 5); `$BOOKS` inside it, empty (person 1) or holding
the demo books (`abo demo --dir $BOOKS/example-ltd`, stopped once it serves);
git's `user.name` and `user.email` set in that `$HOME`. `abo` on the PATH is a
wrapper that runs the release build of the commit under test and appends the
command, its exit status and everything it wrote to a log outside `$HOME`,
and waits for what it copies there before it returns: the first run's
wrapper did not, so output came after the command had ended.
The log is what the ASCII check and the judging read; the person does not
know of it.

Judging is done afterwards, by someone who has read WORKFLOWS.md: each
scenario's books, journal and `git log` against the **Done when** of the
workflows it names.
