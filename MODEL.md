# abo — the model of the books

A proposal, for review before any code. DESIGN.md says how abo is built and
why; WORKFLOWS.md what it is for. This document defines what the books *are*:
a set of states, the invariants every state satisfies, the operations that
take one state to the next, and the one journal text that represents each
state. Where it departs from how abo works today, it says so.

---

## 1. Definitions

**The model** is a set M of states. A state `m ∈ M` is a value of the type
`Books` below whose invariants (§3) all hold.

**The representation** is a function `render: Books → Files`, from a state
to the contents of the text files in the books' directory: `abo.toml`, the
journal, each invoice's content, and `bills/booked`. It is injective: two different states
render to different files. The image of `render` is the set of *canonical*
files. The binary files — bills' documents, invoices' PDFs — are not rendered
but kept as they came or were made; the state holds the hash of each (§2.5).

**Reading** is a function `parse: Files → Books ∪ {Problems}`, with

    parse(render(m)) = m        for every m ∈ M.

On files that are not canonical — a hand edit, books written by an older abo —
`parse` returns the state it can read and the list of places where the files
depart from canonical form, each named by file and line. It never refuses to
read the books on that account (§7).

**An operation** is a partial function `M → M`: defined where its
preconditions hold, and then giving a state that again satisfies every
invariant. Every command abo has — booking, issuing an invoice, recording a
payment, reverting, undoing — is an operation, and nothing changes the books
otherwise.

**hledger compatibility** is the condition that for every `m ∈ M`:

1. hledger reads `render(m)` without error, with `check accounts balanced
   ordereddates` and its balance assertions; and
2. for every account, commodity and period, the balance hledger computes from
   `render(m)` equals the balance the model gives for it (§2.7).

The model may use hledger to compute balances and reports. It may not use
hledger to decide any fact of its own — which entries are invoices, what a
payment pays, whether something is reversed. Those are read by `parse` alone.

Three consequences follow from the definitions, and are why they are chosen:

- A disagreement between two parts of abo about what the books say is
  impossible: there is one `parse`, and every page, command and check reads
  its result.
- A rule is stated once, as an invariant of M, and holds whichever command
  makes the change, because every command is an operation and an operation
  preserves the invariants.
- A text abo did not write is recognisable as such, because it is not
  canonical.

---

## 2. The state

Written as Rust types, which is how they would be written. `Decimal` is an
exact decimal; `Date` a calendar date; `Text` a string holding no character
that `hidden` refuses (by Unicode category, DESIGN §1); `Path` a file's path in
the books' directory; `Hash` a SHA-256 hash; `EntryId` an entry's place in
`entries` (§2.3); `InvoiceNumber` a year and a number in it, `2026-0017`;
`Line` and `PayOut` an invoice's line and a payment option as it printed them,
as today's `.json` holds them.

### 2.1 The books

```rust
struct Books {
    config:         Config,               // the company, VAT method, invoice settings,
                                          // payment options, tax return, closed period
    declarations:   Declarations,         // accounts, commodities, prices
    entries:        Vec<Entry>,           // in the order added; never changed
    statements:     Set<Statement>,       // from the bank, each with the postings it clears
    bill_documents: Set<BillDocument>,    // the files under bills/
}
```

### 2.2 Declarations

```rust
struct Declarations {
    accounts:    Set<Account>,             // the declared accounts
    commodities: Map<Commodity, Places>,   // every commodity with its places
    prices:      Set<Price>,               // market prices, for valuing one in another
}

/// Assets:Bank:PostFinance is Account(["Assets", "Bank", "PostFinance"]).
struct Account(Vec<Segment>);

/// A non-empty Text holding no ':' and no ';', no two spaces in a row, and
/// no space at either end.
struct Segment(Text);

/// EUR, CHF, USD, BTC: three or more capital letters, A to Z (I-K0).
struct Commodity(Text);

/// Digits after the point: `commodity 1000.00 EUR` declares 2, BTC has 8.
struct Places(u8);   // 0 to 18

/// `P 2026-09-30 BTC 60000.00 EUR`: on that date one BTC was worth
/// 60000.00 EUR. hledger reads it to value amounts in another commodity.
struct Price { date: Date, of: Commodity, price: Amount }
```

The hierarchy of accounts is not stored: it is what the set of paths already
says.

| Derived | Definition |
|---|---|
| `class(a)` | its first segment: one of `Assets`, `Liabilities`, `Equity`, `Income`, `Expenses` (I-A0) |
| `parent(a)` | its path less the last segment |
| `under(a, b)` | `a`'s path begins with `b`'s |
| `leaf(a)` | no declared account is under `a` but `a` itself |

What kind of account something is — an asset, a liability — is read from its
class, once, by `parse`. What role an account plays — the receivable, input or
output VAT, where income goes — is read from the configuration, which names
those accounts (I-C3), never from the account's name.

Names are compared exactly, as hledger compares them: `Assets:Bank` and
`Assets:bank` are two accounts. CamelCase segments are the convention the
accounts abo declares follow, not a rule: a numbered chart such as
`Assets:1020:Bank` is an account like any other.

Postings go to leaves alone (I-A2). So the balance of an account is the sum of
the postings to the leaves under it, and a parent's balance is the sum of its
children's — which is how hledger's reports roll accounts up.

Prices are read by two things: the tax return, which values each posting in
another commodity at the price on its day (`--value=then`), and an invoice in
another commodity, which states its VAT in the company's currency at the
latest price on or before its date. A price says what a commodity was worth;
what one was exchanged for in an entry is a posting's cost (§2.3).

A commodity is letters alone, written after the amount: `100.00 EUR`. hledger
also takes symbols such as `$` or `€` and quoted names, written before or after
the amount; its rules for those are where its reading and abo's could part,
and abo already asks for currencies as letters, so the model takes letters
alone.

### 2.3 Entries

Every entry has an **id**: its place in `entries`, counting from 1. Entries are
only appended, and undo removes only the last, so an id is never changed or
reused. Ids replace today's `file:line:date~fingerprint` keys. An
entry names another — a payment the bill it pays, a reversal what it reverses,
a credit note the invoice it cancels — by its id, and so does a link, a log
line or a statement; the id does not move when other entries are booked
around it. Once added, an entry never changes.

```rust
/// Its id is its place in `entries`, from 1.
struct Entry {
    date:        Date,
    description: Text,
    postings:    Vec<Posting>,
    kind:        Kind,
}

struct Posting {
    account: Account,
    amount:  Amount,           // always explicit: no elided amount (§6.4)
    cost:    Option<Amount>,   // what the whole amount was exchanged for
}

struct Amount { quantity: Decimal, commodity: Commodity }
```

**A cost** is what a posting's amount was exchanged for, in another commodity.
An entry balances when, for each commodity, its amounts sum to nought (I-E1);
an exchange of one commodity for another cannot, since neither sums to nought
on its own. With a cost it can: for the balance, a posting with a cost counts
as its cost, and only for the balance — its amount is still what moves in its
account.

```
2026-09-30 Pay Devtools Inc
    Liabilities:Payable      250.00 USD @@ 230.00 EUR
    Assets:Bank             -230.00 EUR
```

The payable is reduced by 250.00 USD, the bank by 230.00 EUR, and the entry
balances: the first posting counts as +230.00 EUR, the second as −230.00 EUR.
This is how a bill in one currency is paid from an account in another, and how
money is changed, or bitcoin bought.

The cost is the total, written `@@`, never a price a unit, `@`: a unit price
multiplied out can leave digits past the commodity's places, which abo and
hledger might round apart; a total leaves nothing to round.

### 2.4 Kinds

What an entry *is* — not a free set of tags — is its kind:

```rust
enum Kind {
    /// Booked by hand: anything that balances.
    Plain,
    /// A supplier's bill. Its document, once filed, names it (§2.5). A debt.
    Bill,
    /// An invoice abo issued: what it says, and the PDF made of it (§2.5).
    /// A debt.
    Invoice { number: InvoiceNumber, content: Content, pdf: Hash },
    /// An invoice's reversal: it cancels the invoice in full, its entry that
    /// invoice's negated, and says what the invoice said, negated — the
    /// content is the invoice's, not stored twice. Numbered in the invoices'
    /// series.
    CreditNote { number: InvoiceNumber, of: EntryId, note: Text, pdf: Hash },
    /// Something taken off one debt, a `Bill` or `Invoice` entry: money paid,
    /// or the rest written off to an account for it — what a customer will
    /// never pay, what a supplier forgave.
    Payment { of: EntryId },
    /// The opposite of another entry, but an invoice's, which is a credit
    /// note.
    Reversal { of: EntryId },
}
```

A **debt** is a `Bill` or `Invoice` entry. Whether one is settled is not
stored: it is settled when its payments have taken off all it owes (§2.7). A
debt is cancelled when it is reversed: a bill's booking by a `Reversal`, an
invoice by a `CreditNote`.

Five things today's tags say are not stored at all, because they follow from
the rest:

- `takes-back:` and `cancels:` are what a reversal *means*: the reversal of a
  payment takes it back, of a bill's booking cancels the bill, of a reversal
  puts back what it reversed. Stored, they could disagree with what they
  follow from; derived, they cannot.
- `settles:` is what the amounts say: a payment settles a debt when it brings
  what is paid up to what is owed. `abo settle --by`, which adds the tag to a
  payment booked without it, is no longer needed.
- `sha256:` belongs to the bill's document (§2.5), not to the entry that books
  it.
- `reconciled:` is the date of a statement, which is not an entry (§2.6).

### 2.5 Bills' documents, and invoices' contents and PDFs

Two kinds of file sit beside the journal, and have almost nothing in common:

| | A bill's document | An invoice's content and PDF |
|---|---|---|
| Comes | from outside: the supplier's PDF, a photograph | from abo, made when it issues the invoice |
| Against its entry | independent: filed before it is booked, or booked before it is filed | made with its entry, in one commit, never apart from it |
| Known by | its hash, which is also the duplicate check | its invoice number |
| Its contents | bytes abo never reads | data abo reads back: lines, customer, due date, how it asked to be paid |

So they are not two kinds of one thing, and the model does not make them one.

**Bills' documents** are the files under `bills/`, each with its hash, and
the bill it is the document of, once that is booked:

```rust
/// A supplier's bill as received — a PDF or a photograph — kept under
/// bills/ as it came: bills/2026/2026-09-21-acme-1a2b3c4d.pdf.
struct BillDocument { path: Path, sha256: Hash, of: Option<EntryId> }
```

The document names its bill, not the bill its document: a document's path
holds its hash, which is not known until the document is there, and a bill may
be booked before. So a booking and its document come in either order, and the
one that comes second records the link — on the document — and no entry
changes, as a statement names the postings it clears.

The set `bill_documents` holds those in the books. It is what three things
are read from:

- **Filed, not booked yet:** a document whose `of` is none. It may be taken
  out again (`unfile`); one that names a bill may not.
- **Booked, document missing:** a `Bill` entry no document names. An ordinary
  state, not an error: filing the document later, for that booking, sets its
  `of`.
- **Booked again:** a document whose bill is reversed may name another, so
  that a bill booked wrongly, reverted, and booked again keeps its document.
- **Duplicates:** filing a document whose hash is in the set already is
  refused.

**An invoice's content** is not a document but the invoice's own data, and
part of its kind:

```rust
/// What an invoice says: today's .json.
struct Content {
    customer:  Text,
    lines:     Vec<Line>,           // quantity, unit price, description, amount
    vat:       InvoiceVat,
    due:       Date,
    pay_to:    Vec<PayOut>,         // the payment options, as printed
    reference: Text,
    note:      Text,
}

/// What an invoice says of VAT.
enum InvoiceVat {
    /// Issued while the company was not VAT registered: it says nothing of VAT.
    NotRegistered,
    /// Registered, charging none — an export, an exempt supply — and why.
    Exempt { reason: Text },
    /// Charged at a rate: rate × net, rounded to the invoice's places. In
    /// another commodity, also the VAT in the company's currency and the
    /// price used.
    Charged { rate: Decimal, amount: Decimal, home: Option<(Amount, Price)> },
}
```

The three cases are the three an invoice can be in, and no other: a reason
comes with no rate, and a rate with no reason. Whether the company is
registered can change; an invoice says what held when it was issued, and is
checked against its own entry, never against today's configuration (I-I3).

**An invoice's PDF** is made once, when the invoice is issued, from its
content and the company's template, and kept as made; a credit note's, from
the invoice's content, negated, and its note. It is not re-made from
the state: the template is the company's to change, and an invoice already
sent must not change with it. The state holds the PDF's hash, so that a PDF
changed or lost since is a problem `parse` reports (§7).

Every invoice in the books is one abo issued, so every invoice has both. An
invoice issued elsewhere — before abo, or by other software — is not an
invoice to the model; should books ever need to hold one, that is the time to
model it.

The rule behind the difference: a document that belongs to exactly one entry
is a hash on that entry; only a bill's document, which exists before or
without its entry, needs a set of its own.

**Supporting documents** — a receipt, a bank's confirmation — are left out of
the model. Should they be wanted, they would be kept as bills' documents are,
and the set and its type renamed then to take them in.

### 2.6 Statements

A statement records what the bank's statement says of an account on a date:
its balance, and which postings it shows. It moves nothing, so it is not an
entry, and the postings it clears are its own, not marks on entries — so that
entries never change, and so that which statement cleared a posting is known.

```rust
/// Its commodity is its balance's.
struct Statement {
    account: Account,
    date:    Date,
    balance: Amount,
    clears:  Set<PostingRef>,   // the postings this statement shows
}

/// Posting `n` of entry `e`, counting from 0.
struct PostingRef { entry: EntryId, n: usize }
```

A posting is cleared by the statement that shows it. A payment dated 28
September that the bank shows only on October's statement is cleared by
October's: September's balance, which does not include it, still agrees with
what September's statement clears (I-S3).

### 2.7 What is derived

Everything else is a function of the state, defined once:

| Derived | Definition |
|---|---|
| `balance(account, commodity, period)` | the sum of the postings, in that commodity and period, to the accounts under it; hledger computes the same (§1) |
| `reversed(e)` | a standing `Reversal` or `CreditNote` names `e` |
| `standing(e)` | `e` is not reversed |
| `account(d)` | the debt's account: the liability a bill's booking credits (I-B2), the asset an invoice's entry debits (I-I3) |
| `owed(d)` | the amount the debt's entry put on `account(d)` |
| `paid(d)` | the sum, over standing payments of `d`, of what each took off `account(d)` |
| `open(d)` | nought if `d` is reversed, else `owed − paid` |
| `settled(d)` | `d` stands, and `open(d)` is nought |
| `next_number(year)` | one past the greatest invoice number of that year |
| `cleared(p)` | some statement clears posting `p` |
| `whole(s)` | every posting to the statement's account, in its commodity, dated on or before it, is cleared by it or an earlier statement: its balance is then the account's, and hledger checks it too (§6.3) |

Because a payment names one debt, `paid` and `open` are always known. The `?`
of today's Bills page cannot occur.

### 2.8 Configuration

`Config` is today's `abo.toml`: the company's name, address and tax number;
the VAT method and its accounts; invoice settings; payment options; the tax
return's boxes; and the date the books are closed through. Its invariants are
in §3.8. It is part of the state, so changing it is an operation like any
other (§4.2).

---

## 3. Invariants

The invariants and conditions below are the rules the five verification runs
kept finding broken one way or another; each fault they found violates one of
them. Under this model each is checked in one place, on the state an operation
produces.

Each is numbered so that tests and refusals can name it. An invariant is a
property of the state alone. Rules that also need the day a change is made, or
today's configuration, are conditions on the change, in §3.9.

### 3.1 Accounts and commodities

- **I-A0** Every declared account's first segment names a class.
- **I-A1** Every account and every commodity that an entry, a statement, a
  price or the configuration names is declared — a commodity with its places.
- **I-A2** Postings go to leaves alone: every account an entry posts to, a
  statement is of, or the configuration names for postings is a `leaf`.
  Declaring an account under one of those breaks it, and is refused; to split
  an account, its balance is moved by an entry to new accounts beside it.
- **I-K0** A commodity is three or more capital letters, A to Z.
- **I-K1** No amount or cost has more digits after the point than its
  commodity's places. Rendered, every amount is written to its places, so
  `10`, `10.0` and `10.00 EUR` are one value and one text.
- **I-K2** At most one price per commodity and date; a price is above nought,
  and in another commodity than its own.

### 3.2 Entries

- **I-E1** Every entry balances: for each commodity, the amounts of its
  postings without a cost and the costs of those with one sum to nought. This
  is how hledger checks it.
- **I-E2** A posting's cost is in another commodity than its amount, and both
  are above nought or both below.
- **I-E3** An entry that names another names one with a smaller id, and is
  dated on or after it. So `reversed` and `standing` (§2.7), defined by each
  other, are defined by induction on the id.

### 3.3 Bills

- **I-B1** A document's `of` names a `Bill` entry, and no two documents name
  the same one.
- **I-B2** A bill's booking credits one liability account, in one commodity.

### 3.4 Invoices

- **I-I1** Invoice numbers — of invoices and credit notes together — are
  unique, and within a year run 1, 2, 3, … with no gap.
- **I-I2** Within a year, numbers follow dates: a greater number is never dated
  before a lesser one.
- **I-I3** An invoice's entry debits one asset account with its total: the
  lines' net, plus the VAT `amount` when it is `Charged`.

### 3.5 Payments

- **I-P1** A payment names a debt that stands. Its posting to the debt's
  account is in the debt's commodity, and takes something off it; when what
  is paid with is in another commodity, that posting carries the cost.
- **I-P2** `paid(d) ≤ owed(d)`, at every debt.

### 3.6 Reversals

- **I-R1** An entry is reversed by at most one standing entry. An invoice is
  reversed by a credit note alone — it is corrected by a credit note, DESIGN
  M3 — and a credit note by nothing.
- **I-R2** A reversal's postings, or a credit note's, are those of what it
  reverses, negated.
- **I-R3** An entry is not reversed while another standing entry, other than
  its reversal, names it. So a bill's booking is not reverted, nor an invoice
  credited, while a payment of it stands: the payment is reverted first.

### 3.7 Statements

- **I-S1** At most one statement per account, commodity and date.
- **I-S2** A statement clears postings that exist, to its account, in its
  commodity, dated on or before it, and cleared by no other statement.
- **I-S3** A statement's balance is the sum of the postings it and the
  account's earlier statements, in its commodity, clear.

### 3.8 Configuration

- **I-C1** The company has a name; no text in the configuration holds a hidden
  character.
- **I-C2** Payment option ids are unique and well formed; each option has what
  its kind needs (DESIGN, A4) and an IBAN or BIC that is valid if given.
- **I-C3** Every account the configuration names is of its kind: money arrives
  in an asset, the receivable is an asset, input VAT is an asset, output and
  flat-rate VAT are liabilities.

### 3.9 Conditions on a change

These hold of each change when it is made, not of the state: they need the
day it is made, or the configuration in force then, and the state remembers
neither for an entry made long ago.

- **N1** No entry or statement is dated after the day it is added.
- **N2** On or before `closed_through`, no entry is added, no statement
  recorded or removed, and no price added or removed.
- **N3** VAT accounts are posted to only as the VAT method allows (DESIGN §6).
- **N4** `pay` moves the money through a money account: an asset, not the
  receivable, not a VAT account. (`write_off` names the account the rest
  goes to.)
- **N5** A `post` entry posts nothing to `account(d)` of any debt `d`: what is
  owed there moves only through bills, invoices, payments and reversals, so
  that what Bills and Invoices show open and those accounts' balances agree.
  What was owed before abo — an opening balance — goes to a leaf of its own,
  beside the receivable, such as `Assets:OpeningReceivable`, which no debt is
  on, and is paid off there by plain entries.
- **N6** `closed_through` only moves forward: a period closed stays closed.

The conditions hold of every operation but `undo`, which takes back the last
one and makes no change of its own: a period closed by mistake is reopened by
undo, while nothing has happened since.

---

## 4. Operations

Each operation is `op(m, args) → m'`, defined when its preconditions hold.
abo checks the invariants on `m'`, and the conditions on the change, before
writing anything; if one fails, the operation is refused with its number and
nothing is written. Most preconditions are therefore not listed: they are the
invariants.

| Operation | Adds or changes |
|---|---|
| `post(date, description, postings)` | a `Plain` entry |
| `file(bytes, date, issuer, for)` | a `BillDocument`; `for`, when given, a `Bill` entry it is the document of |
| `unfile(document)` | removes a `BillDocument` that names no bill: one filed by mistake |
| `bill(document, description, postings)` | a `Bill` entry; `document`, when given — filed already, naming no bill or a reversed one — now names it |
| `invoice(customer, lines, …)` | an `Invoice` entry with its content and PDF, numbered `next_number` |
| `credit(invoice, note)` | a `CreditNote` entry, reversing the invoice, and its PDF |
| `pay(debt, date, amount, from)` | a `Payment` entry, from a money account |
| `write_off(debt, date, to)` | a `Payment` entry of what is open, from the account `to` |
| `revert(entry, date)` | a `Reversal` entry |
| `record(account, date, balance, postings)` | a `Statement` clearing those postings |
| `unrecord(account)` | removes the account's latest statement; an earlier one cannot go, since later ones count what it cleared (I-S3) |
| `config_set(key, value)`, `config_unset(key)` | `config` (§4.2) |
| `account_add(account)`, `account_remove(account)` | `declarations.accounts` |
| `commodity_add(commodity, places)`, `commodity_remove(commodity)` | `declarations.commodities` |
| `price_add(date, commodity, amount)`, `price_remove(date, commodity)` | `declarations.prices` |
| `undo()` | the state before the last operation (§4.3) |

No operation changes an entry once added. Every one adds something; removes a
document that names no bill, or an account's latest statement; links a
document to its bill; or changes the configuration or the declarations.

### 4.1 Declarations

Accounts, commodities and prices are declared by commands, as payment options
are, and listed and removed the same way:

```
abo account   add <Account>                list   remove <Account>
abo commodity add <CODE> <places>          list   remove <CODE>
abo price     add <date> <CODE> <amount>   list   remove <date> <CODE>
```

In the browser, one page lists the accounts, the commodities and the prices,
each with Add and Remove. What each refuses is what the invariants refuse:

| Refused | Because |
|---|---|
| `account add Foo:Bar` | its first segment names no class (I-A0) |
| `account add Assets:Bank:Cash` while `Assets:Bank` holds postings | postings go to leaves alone (I-A2) |
| `account remove` of one an entry, a statement or the configuration names | I-A1 |
| `commodity add eur 2` | three or more capital letters (I-K0) |
| `commodity remove` of one in use | I-A1 |
| `price add` or `price remove` dated on or before `closed_through` | N2 |

There is no edit and no rename. An account's name is in every posting to it,
and entries never change; a commodity's places are in every amount written in
it. So an account that holds postings keeps its name, and a commodity in use
keeps its places; one not yet used is removed and added again, and a balance
is moved to another account by an entry. A price removed changes no invoice:
each holds the price it used.

### 4.2 Configuration

The configuration is read and changed as `git config` does it, by keys named
as in `abo.toml`:

```
abo config list
abo config get   <key>
abo config set   <key> <value>
abo config unset <key>
```

`company.closed_through` closes a period (N6); `vat.method`, `vat.input`,
`invoice.terms_days` and the like set what they name; each box of the tax
return is keys of its own, `report.vat.<code>.label` and `.query`. Every change
is checked by I-C1 to I-C3 and the invariants they bear on, and refused with
the number of the one it would break. `abo company` and `abo payment-options`
stay, to ask what they need as questions; each answer is a `config_set`.

### 4.3 Undo

Undo restores the state before the last operation, and is defined only while
that operation is the last: undo of anything older is not an operation. An
older mistake is taken back with `revert`, which adds the opposite and
removes nothing, as bookkeeping does it.

This replaces today's undo, which reverses the text of any past commit. Over a
state, "the last operation" is well defined; "the text of an old commit,
reversed" is not, and is where the chains the verification runs found came
from: settled twice, paid more than owed, a reversal naming what is gone, a
statement over postings no longer cleared. Undo of an undo is redo of the
last operation, and is allowed while nothing has happened since.

### 4.4 The log

Each operation appends its line to `.abo/log.jsonl` as today: the command and
its stamp. Replaying the lines in order from empty books gives the same state,
and so the same files.

---

## 5. What changes for workflows

Six workflows change shape under the model:

- **Settling** (B6). An invoice or a bill is settled when its payments reach
  what it owes; nothing marks it. A payment booked without naming what it pays
  is reverted and booked again as a payment. A debt that will not be paid in
  full is written off (`write_off`), the rest moved to an account chosen for
  it, so that the payable or receivable and what Bills and Invoices show open
  agree.
- **Declaring an account, a commodity or a price** (A2). By `abo account`,
  `abo commodity` and `abo price`, and their page in the browser, not by
  editing `accounts.journal`, which abo alone writes.
- **Closing a period, and every other setting** (F6). By `abo config`, not by
  editing `abo.toml`; a period closed is not reopened (N6).
- **Undo** (F2). Only the last change, while nothing has happened since.
  Anything older is reverted.
- **Correcting an invoice that has been paid** (F5). Its payments are reverted
  first; then it is credited (I-R3). Today a credit note is taken with
  payments standing, and leaves the receivable below nought.
- **Recording a statement** (D1). Ticking off what the statement shows is part
  of recording it: the ticks stay on the page until **Record**, which commits
  the statement and what it clears together. Once per account, commodity and
  date.

---

## 6. The canonical form

### 6.1 Files

```
abo.toml                     # the configuration, written by render
journal/main.journal         # includes accounts.journal, then each year, in order
journal/accounts.journal     # declarations, written by render
journal/2026.journal         # the year's entries and statements
invoices/2026/2026-0017.json # invoice 2026-0017's content, and its PDF's hash, written by render
invoices/2026/2026-0017.pdf  # made at issue, kept as made
bills/2026/…                 # bills' documents, kept as they came; known by their hash
bills/booked                 # which bill each document is of, written by render
templates/…                  # the company's, not the model's (§9)
```

`render` writes the text files: `abo.toml`, the journal, each invoice's
`.json`, and `bills/booked` — a line for each document that names a bill,
`<entry id> <path>`, in the order of the ids. The two kinds of binary file are
not rendered — a bill's document is kept as it came, an invoice's PDF as it
was made — and the state holds the hash of each, so `parse` can tell one that
is missing or changed.

### 6.2 An entry

```
2026-09-21 Acme Hosting AG, hosting September
    ; id: 412
    ; bill:
    Expenses:Hosting                       100.00 EUR
    Assets:VAT:Input                        10.00 EUR
    Liabilities:Payable                   -110.00 EUR

2026-10-15 Pay Acme Hosting AG, hosting September
    ; id: 431
    ; pays: 412
    Liabilities:Payable                    110.00 EUR
    Assets:Bank                           -110.00 EUR
```

- The date line holds the date, one space, the description, and no status
  mark.
- Then the tags, one a line, `    ; name: value`: `id` first, then the kind's
  own (below), each name in lower case.
- Then the postings, four spaces in, the amount right-aligned at a fixed
  column, every amount written to its commodity's places, the commodity after
  it.
- A posting with a cost has it after the amount: `250.00 USD @@ 230.00 EUR`.
- One blank line after each entry.

The kind is written as tags, and read back from them:

| Kind | Tags |
|---|---|
| `Plain` | none |
| `Bill` | `bill:`, its document named in `bills/booked` |
| `Invoice` | `invoice: <number>` |
| `CreditNote` | `invoice: <number>`, `credits: <id>` |
| `Payment` | `pays: <id>` |
| `Reversal` | `reverses: <id>` |

An entry names another by its id. What that id is — an invoice's number, a
bill's issuer — is in the description abo writes for the entry ("Customer One
pays invoice 2026-0017"), for a person reading the journal.

### 6.3 A statement

A statement is written as an entry of one posting of nought, without an id,
naming each posting it clears as `<entry>.<n>`, one a line:

```
2026-09-30 Statement of Assets:Bank
    ; statement: 457.50 EUR
    ; clears: 412.2
    ; clears: 431.1
    Assets:Bank                              0.00 EUR = 457.50 EUR
```

The `= 457.50 EUR` — hledger's balance assertion — is written when the
statement is `whole` (§2.7). Then the statement's balance is the account's, and
hledger checks it on every reading: a second computation of I-S3, independent
of abo's, so that a fault in either shows as a disagreement. Otherwise it is
left off, and the model alone checks the statement.

Clearing is written here alone. hledger's own mark for it, a `*` on the
posting, is not used: writing it would change an old entry's text each time a
later statement cleared one of its postings, and an entry's text is fixed when
it is added.

### 6.4 Order and form

- A year's entries and statements are in date order; on one date, entries by
  id, then statements.
- Every posting's amount is written. Today one may be left off and hledger
  infers it; then abo's reading of an amount and hledger's could differ.
  Written out, they cannot.
- A cost is a total, `@@`; a unit price, `@`, is not canonical.
- Tag names are in lower case. hledger matches tag names regardless of case,
  so `Invoice:` would be an invoice to hledger and not to abo: canonical form
  has only lower case, and `parse` reports anything else.
- Nothing but entries and comments abo writes: no `include` but main's, no
  `alias`, no automated or periodic transactions, no hledger directive that
  changes how what follows is read.

### 6.5 abo.toml

Written by `render` from `config`, keys in a fixed order, values
in their canonical forms (an IBAN in groups of four, a BIC in capitals). Only
abo writes it: every change goes through `abo config`, `abo company` and `abo
payment-options`, a comment written in it by hand is not kept, and any other
hand edit is a problem `parse` reports (§7).

---

## 7. Reading what abo did not write

`parse` reads every file into a state and a list of problems. A problem is a
place — file and line — and what is wrong with it: not canonical (spacing, a
tag name in capitals, an elided amount), not readable as a kind, or breaking an
invariant (a payment of two debts, an invoice number twice).

- **Reading never stops on a problem.** Pages and reports show the books, and
  every page says that the books hold problems, with a link to the list.
- **No operation writes on top of a problem it would touch.** One that touches
  none of the places named goes ahead; one that would is refused, naming them.
- **`abo check`** lists the problems; **`abo tidy`** rewrites what is merely not
  canonical — spacing, order, amount places, lower-case tag names — into
  canonical form, as one commit, and leaves the rest for a person to resolve.

This replaces both today's refusal to read an `abo.toml` it would not have
written, and its sweeping a hand edit into its own commit.

---

## 8. Books from before

Books written before the model — under rules 1 to 4 — are not migrated: no real
books were written then, only the demo, which is rebuilt on every start, and
the tests', which are rebuilt with the code. `parse` reads such books as far as
it can and reports the rest as not canonical (§7). Should real books from before
ever turn up, a converter is written then, for what they hold.

With the model, one set of invariants holds, and the 76 rule-version switches go.

---

## 9. What the model does not cover

- **Templates** (`templates/*.typ`) are the company's, edited by hand, and not
  part of the state. An invoice's content and its PDF's hash are.
- **The log** and the rest of `.abo/` are working state, as today.
- **Reports** are functions of the state, not part of it.
- **Users** (M5) would add an author to each operation, not new state.

---

## 10. Testing

Two properties, tested over random sequences of operations on fresh books:

- **T1** After every operation that was not refused, every invariant holds.
- **T2** For every state reached, `parse(render(m)) = m`; hledger reads
  `render(m)`, its balance assertions included, and its balances equal the
  model's. So every statement that is `whole` is checked by hledger as well as
  by abo, at every step of every sequence.

A generator draws operations — post, file, unfile, bill, invoice, credit,
pay, write-off, revert, record, unrecord, config set, the declarations' add
and remove, undo — with
arguments drawn from the current state: real debts and ids most of the time,
wrong ones some of the time. A failing sequence is shrunk to the shortest that
fails and kept as a regression test.

The 172 regression tests stay; most become tests of one invariant each.
