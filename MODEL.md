# abo — the model of the books

DESIGN.md says how abo is built and why; WORKFLOWS.md what it is for. This
document defines what the books *are*: a set of states, the invariants every
state satisfies, the operations that take one state to the next, and the one
text that represents each state. Where it departs from how abo works today, it
says so.

The definitions are also code: `src/model`, whose types, `check`, `apply`,
`render` and `parse` are the ones below, and whose tests are the claims this
document makes (§10). Where the two differ, the code is wrong or this is, and
a test is what says which.

---

## 1. Definitions

**The model** is a set M of states. A state `m ∈ M` is a value of the type
`Books` below whose invariants (§3) all hold.

**The representation** is a function `render: Books → Files`, from a state
to the contents of the text files in the books' directory: `abo.toml`, the
journal, each invoice's `.json`, and `bills/index`. It is injective: two
different states render to different files. The image of `render` is the set
of *canonical* files. The binary files — bills' documents, invoices' PDFs —
are not rendered but kept as they came or were made; the state holds the hash
of each (§2.5).

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

**Each fact is held once.** A field of the state is one of two things:

- an *input*: something said from outside that nothing else in the state
  determines — a posting a person wrote, an invoice's lines, a document's
  date; or
- a *check*: a value from outside that the state also determines, kept to be
  compared with it — a statement's balance, which the bank states and the
  cleared postings sum to; a PDF's or a document's hash, which the file on
  disk has.

Everything else is a function of the state (§2.7): an invoice's number, its
amounts and its postings; a reversal's postings; a document's path; whether
a debt is settled. A value stored twice can disagree with itself, and needs an
invariant to say it does not; a value derived cannot. So an invariant (§3)
says only that an input is well formed, or that a check agrees.

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

As `src/model/mod.rs` writes it. `Decimal` is an exact decimal; `Date` a
calendar date; a text holds no character that `hidden` refuses (by Unicode
category, DESIGN §1), and a newline only where the field is lines; `Hash` a
SHA-256 hash, in lower-case hex; `EntryId` an entry's place in `entries`
(§2.3).

### 2.1 The books

```rust
struct Books {
    config:      Config,                                      // abo.toml (§2.8)
    accounts:    Set<Account>,                                // the declared accounts
    commodities: Map<Commodity, Places>,                      // every commodity, with its places
    prices:      Map<(Commodity, Date), Amount>,              // what one unit was worth, in another
    entries:     Vec<Entry>,                                  // in the order added; never changed
    statements:  Map<(Account, Commodity, Date), Statement>,  // from the bank
    documents:   Map<Hash, Document>,                         // bills' documents, by their hash
}
```

A map's key is what identifies its value, so what would otherwise be a rule —
one price per commodity and date, one statement per account, commodity and
date, no document filed twice — is how the state is built, and needs no
invariant.

### 2.2 Accounts, commodities and prices

`Account` is a path of segments joined by `:`: `Assets:Bank:PostFinance`. A
segment is a non-empty text holding no `:` and no `;`, no space but U+0020 —
hledger reads every other, a no-break space among them, as that one — no two
spaces in a row, and no space at either end. `Commodity` is three or more capital letters, A to
Z: EUR, CHF, BTC. `Places` is the digits after the point, 0 to 16:
`commodity 1000.00 EUR` declares 2, BTC has 8. A price `P 2026-09-30 BTC
60000.00 EUR` says that on that date one BTC was worth 60000.00 EUR; hledger
reads it to value amounts in another commodity.

The hierarchy of accounts is not stored: it is what the set of paths already
says.

| Derived | Definition |
|---|---|
| `class(a)` | its first segment: one of `Assets`, `Liabilities`, `Equity`, `Income`, `Expenses` (I-A0) |
| `under(a, b)` | `a`'s path begins with `b`'s |
| `leaf(a)` | no declared account is under `a` but `a` itself |

What kind of account something is — an asset, a liability — is read from its
class. What role an account plays — the receivable, input or output VAT, where
income goes — is read from the configuration, which names those accounts
(I-C3), never from the account's name.

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
latest price on or before its date, and keeps that price. A price says what a
commodity was worth; what one was exchanged for in an entry is a posting's
cost (§2.3).

A commodity is letters alone, written after the amount: `100.00 EUR`. hledger
also takes symbols such as `$` or `€` and quoted names, written before or after
the amount; its rules for those are where its reading and abo's could part,
and abo already asks for currencies as letters, so the model takes letters
alone.

### 2.3 Entries

Every entry has an **id**: its place in `entries`, counting from 1. Entries are
only appended, and undo removes only the last, so an id is never changed or
reused. An entry names another — a payment the debt it pays, a reversal what
it reverses, a credit note the invoice it cancels — by its id, and so does a
document or a statement; the id does not move when other entries are booked
around it. Once added, an entry never changes.

```rust
struct Entry {
    date:        Date,
    description: Text,   // not empty; no ';'; not begun by '*', '!' or '('; no space at either end
    kind:        Kind,
}

struct Posting {
    account: Account,
    amount:  Amount,
    cost:    Option<Amount>,   // what the whole amount was exchanged for
}

struct Amount { quantity: Decimal, commodity: Commodity }
```

A description is what the date line holds after the date. The rules on it are
the ones under which hledger reads it back as written: `;` would begin a
comment, and `*`, `!` and `(` a status or a code.

**A cost** is what a posting's amount was exchanged for, in another commodity.
An entry balances when, for each commodity, its amounts sum to nought (I-E1);
an exchange of one commodity for another cannot, since neither sums to nought
on its own. With a cost it can: for the balance, a posting with a cost counts
as its cost, and only for the balance — its amount is still what moves in its
account.

```
2026-09-30 Pay Devtools Inc
    Liabilities:Payable                          250.00 USD @@ 230.00 EUR
    Assets:Bank                                 -230.00 EUR
```

The payable is reduced by 250.00 USD, the bank by 230.00 EUR, and the entry
balances: the first posting counts as +230.00 EUR, the second as −230.00 EUR.
This is how a bill in one currency is paid from an account in another, and how
money is changed, or bitcoin bought.

The cost is the total, written `@@`, never a price a unit, `@`: a unit price
multiplied out can leave digits past the commodity's places, which abo and
hledger might round apart; a total leaves nothing to round. A cost has its
amount's sign (I-E2); hledger takes it from the amount, so it is written
without one.

### 2.4 Kinds

What an entry *is* is its kind. Only the kinds whose postings a person writes
hold them; the others hold what their postings follow from.

```rust
enum Kind {
    /// Booked by hand: anything that balances.
    Plain(Vec<Posting>),
    /// A supplier's bill. A debt.
    Bill(Vec<Posting>),
    /// An invoice abo issued: what it says, and its PDF's hash (a check).
    /// A debt.
    Invoice { content: Content, pdf: Hash },
    /// Cancels invoice `of` in full. Numbered in the invoices' series.
    CreditNote { of: EntryId, note: Text, pdf: Hash },
    /// Something taken off debt `of`, a `Bill` or `Invoice` entry: money
    /// paid, or the rest written off to an account for it.
    Payment { of: EntryId, postings: Vec<Posting> },
    /// The opposite of `of`.
    Reversal { of: EntryId },
}
```

`postings(e)` is the entry's own postings for `Plain`, `Bill` and `Payment`.
For the others it is defined:

- **An invoice's**: the receivable debited with the total; then income
  credited with the net and output VAT with the VAT, or, where the invoice
  books no output VAT (`accounts.output` is none), income credited with the
  total.
- **A reversal's, and a credit note's**: those of the entry it names, negated.

The amounts are those of §2.5. An entry so defined balances by construction,
and cannot disagree with what the invoice says or with what it reverses.

A **debt** is a `Bill` or `Invoice` entry. Whether one is settled is not
stored: it is settled when its payments have taken off all it owes (§2.7). A
debt is cancelled when it is reversed: a bill's booking by a `Reversal`, an
invoice by a `CreditNote`.

What today's tags say is derived, not stored:

- `takes-back:` and `cancels:` are what a reversal *means*: the reversal of a
  payment takes it back, of a bill's booking cancels the bill, of a reversal
  puts back what it reversed.
- `settles:` is what the amounts say: a payment settles a debt when it brings
  what is paid up to what is owed. `abo settle --by`, which adds the tag to a
  payment booked without it, is no longer needed.
- `sha256:` belongs to the bill's document (§2.5), not to the entry that books
  it.
- `reconciled:` is the date of a statement, which is not an entry (§2.6).
- `invoice:` — the number — is the invoice's place in its year (§2.7).

### 2.5 Bills' documents, and invoices' contents and PDFs

Two kinds of file sit beside the journal, and have almost nothing in common:

| | A bill's document | An invoice's content and PDF |
|---|---|---|
| Comes | from outside: the supplier's PDF, a photograph | from abo, made when it issues the invoice |
| Against its entry | independent: filed before it is booked, or booked before it is filed | made with its entry, in one commit, never apart from it |
| Known by | its hash, which is also the duplicate check | its entry |
| Its contents | bytes abo never reads | data abo reads back: lines, customer, due date, how it asked to be paid |

So they are not two kinds of one thing, and the model does not make them one.

**A bill's document** is a file under `bills/`, kept as it came, and known by
its hash:

```rust
struct Document {
    date: Date,
    name: Text,              // the issuer, as the path says it: a-z, 0-9 and '-'
    ext:  Text,              // pdf, jpg: a-z and 0-9
    of:   Option<EntryId>,   // the bill it is the document of, once booked
}
```

Its path follows from these and its hash:
`bills/<year>/<date>-<name>-<first 8 digits of the hash>.<ext>`. The hash is a
check: the file at that path has it, or `parse` reports it (§7).

The document names its bill, not the bill its document: a booking and its
document come in either order, and the one that comes second records the link
— on the document — so that no entry changes, as a statement names the
postings it clears. What the set of documents says:

- **Filed, not booked yet:** a document whose `of` is none. It may be taken
  out again (`unfile`); one that names a bill may not.
- **Booked, document missing:** a `Bill` entry no document names. An ordinary
  state, not an error: filing the document later, for that booking, sets its
  `of`.
- **Booked again:** a document whose bill is reversed may name another, so
  that a bill booked wrongly, reverted, and booked again keeps its document.
- **Duplicates:** filing a document whose hash is filed already is refused.

**An invoice's content** is what it says. Its amounts are not in it: they
follow.

```rust
struct Content {
    customer:  Lines,                 // name, then address
    commodity: Commodity,
    lines:     Vec<Line>,
    vat:       InvoiceVat,
    due:       Date,
    pay_to:    Vec<PayOut>,           // the payment options, as printed
    reference: Text,
    note:      Lines,
    accounts:  InvoiceAccounts,       // what its entry posts to
}

struct Line { description: Text, quantity: Decimal, unit_price: Decimal }

struct PayOut { label: Text, lines: Vec<Text> }

/// As configured when it was issued.
struct InvoiceAccounts { receivable: Account, income: Account, output: Option<Account> }

enum InvoiceVat {
    /// Issued while the company was not VAT registered: it says nothing of VAT.
    NotRegistered,
    /// Registered, charging none — an export, an exempt supply — and why.
    Exempt { reason: Text },
    /// Charged at `rate` percent. In another commodity than the company's,
    /// the price its VAT was stated at in the company's.
    Charged { rate: Decimal, home: Option<Price> },
}

struct Price { date: Date, price: Amount }
```

The amounts, each rounded half away from nought to the commodity's places:

    amount(line) = quantity × unit_price
    net          = Σ amount(line)
    vat          = net × rate / 100    when Charged, else nought
    total        = net + vat

Everything an invoice depends on that can change later — the configured
accounts, the VAT rate, the payment options, a price — it holds as it was when
issued. Those are inputs to it, not copies: the configuration says what holds
now, the invoice what held then. A commodity's places do not change while
anything is in it (§4.1), so the amounts, computed again, are always the ones
printed.

**An invoice's PDF** is made once, when the invoice is issued, from its content
and the company's template, and kept as made; a credit note's from the
invoice's content, negated, and its note. It is not made again: the template
is the company's to change, and an invoice already sent must not change with
it. Its hash is a check, so that a PDF changed or lost since is a problem
`parse` reports (§7).

Every invoice in the books is one abo issued, so every invoice has both. An
invoice issued elsewhere — before abo, or by other software — is not an
invoice to the model; should books ever need to hold one, that is the time to
model it.

**Supporting documents** — a receipt, a bank's confirmation — are left out of
the model. Should they be wanted, they would be kept as bills' documents are.

### 2.6 Statements

A statement records what the bank's statement says of an account in a
commodity on a date: its balance, and which postings it shows. It moves
nothing, so it is not an entry, and the postings it clears are its own, not
marks on entries — so that entries never change, and so that which statement
cleared a posting is known.

```rust
/// Keyed by its account, commodity and date.
struct Statement {
    balance: Decimal,          // what the bank says: a check
    clears:  Set<PostingRef>,  // the postings it shows
}

/// Posting `n` of entry `entry`, counting from 0.
struct PostingRef { entry: EntryId, n: usize }
```

A posting is cleared by the statement that shows it. A payment dated 28
September that the bank shows only on October's statement is cleared by
October's: September's balance, which does not include it, still agrees with
what September's statement clears (I-S3).

### 2.7 What is derived

| Derived | Definition |
|---|---|
| `postings(e)` | §2.4 |
| `amount`, `net`, `vat`, `total` | of an invoice: §2.5 |
| `number(e)` | of an invoice or credit note: its year, and its place among that year's invoices and credit notes, by id: `2026-0017` |
| `path(document)` | §2.5 |
| `balance(account, commodity, period)` | the sum of the postings, in that commodity and period, to the accounts under it; hledger computes the same (§1) |
| `reversed(e)` | a standing `Reversal` or `CreditNote` names `e` |
| `standing(e)` | `e` is not reversed |
| `account(d)` | the debt's account: the one liability a bill's booking credits (I-B2), an invoice's receivable |
| `owed(d)` | what the debt's entry put on `account(d)`: the credit, or the total |
| `paid(d)` | the sum, over standing payments of `d`, of what each took off `account(d)` |
| `open(d)` | nought if `d` is reversed, else `owed − paid` |
| `settled(d)` | `d` stands, and `open(d)` is nought |
| `cleared(p)` | some statement clears posting `p` |
| `whole(s)` | every posting to the statement's account, in its commodity, dated on or before it, is cleared by it or an earlier statement: its balance is then the account's, and hledger checks it too (§6.3) |

`reversed` and `standing` are defined by each other; since a reverser has a
greater id than what it reverses (I-E3), they are defined by induction on the
id, from the last entry down.

An invoice's number is its place, not a stored value: the numbers of a year
run 1, 2, 3, … with no gap and no repeat because they are counted, not
assigned. Undo of the last invoice frees its number; nothing else removes one.

Because a payment names one debt, `paid` and `open` are always known. The `?`
of today's Bills page cannot occur.

### 2.8 Configuration

`Config` is `abo.toml`: the company's name, address, tax number and currency,
and the date the books are closed through; the VAT method and its accounts;
the receivable, income, terms and VAT rate invoices are issued with; the
payment options, by id; and the tax return's boxes, by code. Its invariants
are in §3.8. It is part of the state, so changing it is an operation like any
other (§4.2).

---

## 3. Invariants

The invariants and conditions below are the rules the five verification runs
kept finding broken one way or another; each fault they found violates one of
them. Under this model each is checked in one place, `check`, on the state an
operation produces.

Each is numbered so that tests and refusals can name it. An invariant is a
property of the state alone, and says that an input is well formed or that a
check agrees (§1). Rules that also need the day a change is made, or today's
configuration, are conditions on the change, in §3.9. Numbers once used are
not reused: those missing below were rules about a value now derived.

### 3.1 Accounts and commodities

- **I-A0** Every declared account's segments are well formed, and its first
  names a class.
- **I-A1** Every account and every commodity that an entry, a statement, a
  price, an invoice or the configuration names is declared.
- **I-A2** Postings go to leaves alone: every account an entry posts to, a
  statement is of, or the configuration names for postings is a `leaf`.
  Declaring an account under one of those breaks it, and is refused; to split
  an account, its balance is moved by an entry to new accounts beside it.
- **I-K0** A commodity is three or more capital letters, A to Z, with 0 to 16
  places: with the 12 digits I-K3 allows before the point, 28, all an exact
  decimal holds.
- **I-K1** No amount, cost, price or statement balance has more digits after
  the point than its commodity's places.
- **I-K2** A price is above nought, and in another commodity than its own.
- **I-K3** Every number the state holds — an amount, a cost, a price, a
  statement's balance, an invoice line's quantity and unit price — is below
  10^12 in magnitude, and every VAT rate is 0 to 100 percent. Within these,
  every sum and product the model takes fits an exact decimal (28 digits).
  Checked first, and alone: the rest computes with these numbers.
- **I-D1** Every date is in the years 1 to 9999, which the files write as four
  digits and hledger reads. Checked first, and alone, with I-K3.

### 3.2 Entries

- **I-E0** Every description is one the date line can hold (§2.3).
- **I-E1** Every entry has two postings or more, and balances: for each
  commodity, the amounts of its postings without a cost and the costs of those
  with one sum to nought. This is how hledger checks it.
- **I-E2** A posting's cost is in another commodity than its amount, and both
  are above nought or both below.
- **I-E3** An entry that names another names one with a smaller id, and is
  dated on or after it.

### 3.3 Bills

- **I-B1** A document's name and extension are well formed, and no two
  documents have one path — which takes two of one issuer and day whose hashes
  share their first 32 bits; its `of` names a `Bill` entry, and no two
  documents name the same one.
- **I-B2** A bill's booking credits exactly one liability account.

### 3.4 Invoices

- **I-I1** An invoice has a line or more, a total above nought, a customer
  named on the first line,
  and a due date on or after its own. It books output VAT only when it charges
  VAT; its receivable is an asset, its income an income account, its output VAT
  a liability. An exempt invoice says why. A home price is above nought and in
  another commodity. A credit note's `of` is an invoice.
- **I-I2** Within a year, invoices and credit notes are dated in the order of
  their ids, so their numbers follow their dates.

### 3.5 Payments

- **I-P1** A standing payment names a debt that stands, and has exactly one
  posting to the debt's account: in the debt's commodity, taking something
  off it. Paid in another commodity, that posting carries the cost.
- **I-P2** `paid(d) ≤ owed(d)`, at every debt that stands.
- **I-P3** An account a debt is on — `account(d)` of any debt `d` — is posted
  to only by a debt's own posting there, by a payment's posting to its debt's
  account, and by their reversals: an entry has at most one posting to such an
  account. So on each of them, the balance is what the debts there have open
  — what Bills and Invoices show open and the books agree. What was owed
  before abo — an opening balance — goes to a leaf of its own, beside the
  receivable, such as `Assets:OpeningReceivable`, which no debt is on, and is
  paid off there by plain entries.

  This was N5, a condition on `post`: it looked only at the debts there were
  when the entry was posted, so an entry to the receivable before the first
  invoice passed it, and the balance and what is open disagreed from the
  first invoice on. The property test of §10 found it.

### 3.6 Reversals

- **I-R1** An entry is reversed by at most one standing entry. An invoice is
  reversed by a credit note alone — it is corrected by a credit note, DESIGN
  M3 — and a credit note by nothing.
- **I-R3** An entry is not reversed while another standing entry, other than
  its reversal, names it. So a bill's booking is not reverted, nor an invoice
  credited, while a payment of it stands: the payment is reverted first.

### 3.7 Statements

- **I-S2** A statement clears postings that exist, to its account, in its
  commodity, dated on or before it, and cleared by no other statement.
- **I-S3** A statement's balance is the sum of the postings it and the
  account's earlier statements, in its commodity, clear.

### 3.8 Configuration

- **I-C1** The company has a name; no text in the configuration holds a hidden
  character.
- **I-C2** Payment option ids are well formed; each option has what its kind
  needs (DESIGN, A4) and an IBAN or BIC that is valid, in its canonical form,
  if given.
- **I-C3** Every account the configuration names is of its kind: money arrives
  in an asset, the receivable is an asset, income is income, input VAT is an
  asset, output and flat-rate VAT are liabilities. A flat rate is above
  nought; terms and the VAT rate are not below it.

### 3.9 Conditions on a change

These hold of each change when it is made, not of the state: they need the
day it is made, or the configuration in force then, and the state remembers
neither for an entry made long ago.

- **N1** No entry or statement is dated after the day it is added.
- **N2** On or before `closed_through`, no entry is added, no statement
  recorded or removed, and no price added or removed.
- **N3** VAT accounts are posted to only as the VAT method allows (DESIGN §6):
  a bill posts nothing to output VAT, nor, under a flat rate, to the flat
  rate's account. An invoice books output VAT as the method in force says,
  which `invoice` does by construction.
- **N4** `pay` moves the money through a money account: an asset, not the
  receivable, not input VAT. (`write_off` names the account the rest goes
  to.)
- **N5** is now I-P3.
- **N6** `closed_through` only moves forward: a period closed stays closed.

The conditions hold of every operation but `undo`, which takes back the last
one and makes no change of its own: a period closed by mistake is reopened by
undo, while nothing has happened since.

---

## 4. Operations

Each operation is `op(m, args) → m'`, defined when its preconditions hold.
`apply` builds `m'`, then checks the invariants on it (`check`) and the
conditions on the change, before anything is written; if one fails, the
operation is refused with its number and nothing is written. Most
preconditions are therefore not listed: they are the invariants. The few that
are not — the debt paid is a debt, the statement removed exists — refuse as
`OP`.

| Operation | Adds or changes |
|---|---|
| `post(date, description, postings)` | a `Plain` entry |
| `file(bytes, date, issuer, for)` | a `Document`, by the bytes' hash; `for`, when given, a `Bill` entry it is the document of |
| `unfile(document)` | removes a `Document` that names no bill: one filed by mistake |
| `bill(document, description, postings)` | a `Bill` entry; `document`, when given — filed already, naming no bill or a reversed one — now names it |
| `invoice(customer, lines, rate, …)` | an `Invoice` entry with its content and its PDF's hash; the accounts, terms, payment options and, in another commodity, the price, as configured and declared now |
| `credit(invoice, note)` | a `CreditNote` entry, reversing the invoice, and its PDF |
| `pay(debt, date, amount, from, cost)` | a `Payment` entry, from a money account; `cost`, what was paid, when in another commodity |
| `write_off(debt, date, to)` | a `Payment` entry of what is open, from the account `to` |
| `revert(entry, date)` | a `Reversal` entry |
| `record(account, commodity, date, balance, postings)` | a `Statement` clearing those postings |
| `unrecord(account, commodity)` | removes the latest statement of the account in the commodity; an earlier one cannot go, since later ones count what it cleared (I-S3) |
| `config_set(key, value)`, `config_unset(key)` | `config` (§4.2) |
| `account_add(account)`, `account_remove(account)` | `accounts` |
| `commodity_add(commodity, places)`, `commodity_remove(commodity)` | `commodities` |
| `price_add(date, commodity, amount)`, `price_remove(date, commodity)` | `prices` |
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

`company.closed_through` closes a period (N6); `vat.input`,
`invoice.terms_days` and the like set what they name; each payment option and
each box of the tax return is keys of its own, `payment.<id>.iban`,
`report.vat.<code>.label` and `.query`. A value is read as the first of a
table, a string or a whole number that the key takes, so a value `{ … }` sets
a table's keys together — for what cannot change one key at a time, the VAT
method and its accounts — and an address that begins with `{` is still an
address:

```
abo config set vat '{ method = "flat", rate = "6.5", account = "Liabilities:VAT" }'
```

Every change is checked by I-C1 to I-C3 and the invariants they bear on, and
refused with the number of the one it would break; a key that names nothing,
or a value of the wrong kind, is refused as `abo.toml` would not read. `abo company` and `abo payment-options`
stay, to ask what they need as questions; each answer is a `config_set`.

### 4.3 Undo

Undo restores the state before the last operation, and is defined only while
that operation is the last: undo of anything older is not an operation. An
older mistake is taken back with `revert`, which adds the opposite and
removes nothing, as bookkeeping does it. In the model, undo is not a function
of the state: it is the state before, which whoever holds the states keeps —
the git history, in the books.

This replaces today's undo, which reverses the text of any past commit. Over a
state, "the last operation" is well defined; "the text of an old commit,
reversed" is not, and is where the chains the verification runs found came
from: settled twice, paid more than owed, a reversal naming what is gone, a
statement over postings no longer cleared. Undo of an undo is redo of the
last operation, and is allowed while nothing has happened since.

### 4.4 The log

Each operation appends its line to `.abo/log.jsonl` as today: the command and
its stamp. The log is a record of what was done, not a source of the state:
the state is read from the files alone. Replaying it needs what the commands
read besides — the documents filed, the templates as they were — so the git
history, not the log, is what restores a state.

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
invoices/2026/2026-0017.json # invoice 2026-0017's content and its PDF's hash, written by render
invoices/2026/2026-0017.pdf  # made at issue, kept as made
bills/2026/…                 # bills' documents, kept as they came; known by their hash
bills/index                  # every document: its hash, path and bill, written by render
templates/…                  # the company's, not the model's (§9)
```

`render` writes the text files: `abo.toml`, the journal, each invoice's and
credit note's `.json`, and `bills/index` — a line for each document,
`<hash> <path> <entry id or ->`, in the order of the hashes. An invoice's
`.json` holds its content and its PDF's hash; a credit note's, its note and its
PDF's hash. The two kinds of binary file are not rendered — a bill's document
is kept as it came, an invoice's PDF as it was made — and the state holds the
hash of each, so `parse` can tell one that is missing or changed.

### 6.2 An entry

```
2026-09-21 Acme Hosting AG, hosting September
    ; id: 412
    ; bill:
    Expenses:Hosting                              100.00 EUR
    Assets:VAT:Input                               10.00 EUR
    Liabilities:Payable                          -110.00 EUR

2026-10-15 Pay Acme Hosting AG, hosting September
    ; id: 431
    ; pays: 412
    Liabilities:Payable                           110.00 EUR
    Assets:Bank                                  -110.00 EUR
```

- The date line holds the date, one space, the description, and no status
  mark.
- Then the tags, one a line, `    ; name: value`: `id` first, then the kind's
  own (below), each name in lower case.
- Then the postings — for every kind, those it holds and those it derives —
  four spaces in, the amount ending at column 60 or two spaces after a longer
  account, every amount written to its commodity's places, the commodity after
  it.
- A posting with a cost has it after the amount, unsigned: `-250.00 USD @@
  230.00 EUR`.
- One blank line after each entry.

The kind is written as tags, and read back from them:

| Kind | Tags |
|---|---|
| `Plain` | none |
| `Bill` | `bill:` |
| `Invoice` | `invoice: <number>`, its content in its `.json` |
| `CreditNote` | `invoice: <number>`, `credits: <id>` |
| `Payment` | `pays: <id>` |
| `Reversal` | `reverses: <id>` |

An entry names another by its id. What that id is — an invoice's number, a
bill's issuer — is in the description abo writes for the entry ("Pay Invoice
2026-0017 | Customer One"), for a person reading the journal. A description abo
writes begins with a word of its own — Invoice, Credit note, Pay, Write off,
Revert — so that a customer named `(Acme) GmbH` or `*Star Ltd` is not read by
hledger as a code or a status; a `;` in a name is written `,` there, and kept
as given in the invoice's content.

A derived value written out — an invoice's postings and number, a reversal's
postings, a statement's assertion — is written for hledger and for a person
reading; `parse` does not read it as a fact. A file whose written value
differs from the derived one is not canonical (§7).

### 6.3 A statement

A statement is written as an entry of one posting of nought, without an id,
naming each posting it clears as `<entry>.<n>`, one a line:

```
2026-09-30 Statement of Assets:Bank
    ; statement: 457.50 EUR
    ; clears: 412.2
    ; clears: 431.1
    Assets:Bank                                     0.00 EUR = 457.50 EUR
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
  id, then statements by account and commodity.
- A commodity is declared with a decimal point, and its places after it:
  `commodity 1000.00 EUR`, `commodity 1000. JPY`. hledger reads no other.
- Years are written with four digits.
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
in their canonical forms (an IBAN in groups of four, a BIC in capitals). In
`abo.toml` and the `.json` files a number is written in its shortest form,
`8.1`, not `8.10`, so that one value is one text. Only
abo writes it: every change goes through `abo config`, `abo company` and `abo
payment-options`, a comment written in it by hand is not kept, and any other
hand edit is a problem `parse` reports (§7).

---

## 7. Reading what abo did not write

`read` reads every file into a state and a list of problems. A problem is a
place — file and line — and what is wrong with it: not readable (`parse` fails
there), breaking an invariant (`check` names it), or not canonical. Files are
canonical exactly when `render(parse(files)) = files`, so that last list needs
no rules of its own: it is where the two differ.

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

Every state an operation produces passes `check`, by construction: `apply`
keeps no other. What the tests check is what construction does not give. They
are properties (`src/model/tests.rs`, with proptest), each over plans of up to
60 operations on fresh books:

| Property | Claim |
|---|---|
| **The text** | |
| `files_say_the_state` (T1) | for every state reached, `parse(render(m)) = m`, and `read` finds the files canonical and sound; as `parse` is a function, `render` is injective (§1) |
| `edits_are_noticed` | an edit to canonical files — a character inserted, deleted or recased, a long number, a whole file replaced by anything — reads as another state or with a problem, and reading never panics (§7) |
| `rendered_text_is_tidy` | no tab, no carriage return, no space at a line's end, no two blank lines running, a newline at the end (§6) |
| `amounts_are_written_to_their_places` | every amount, cost, price and statement balance in the journal is written to its commodity's places (§6.2) |
| `the_journal_is_in_order` | a year's journal is in date order; on one date, entries by id, then statements by account and commodity (§6.4) |
| `the_files_are_exactly_the_states` | a `.json` for each invoice and credit note and no other, a line in `bills/index` for each document, an include for each year that has anything, in order; the tags are abo's own, in lower case (§6.1, §6.4) |
| `tidying_is_idempotent` | `abo tidy`, which is `render(parse(f))`, gives text that reads back as the same state, and tidied again is unchanged — whatever files it read, rules broken or not; what is left to report is what breaks a rule (§7) |
| `problems_name_lines_that_are_there` | each problem names a line of its file that is there (§7) |
| **hledger** | |
| `hledger_agrees` (T2) | hledger reads `render(m)` — `check accounts commodities balanced ordereddates`, balance assertions included — and its balance of every account in every commodity is the model's; so every `whole` statement is checked by hledger as well (§6.3) |
| `hledger_agrees_through_any_date` | the same, through the date of any entry (§1: every period) |
| `hledger_reads_entries_as_written` | hledger reads every entry's date, description and tags as written, and each posting's account and amount, and at cost its cost — so it and abo agree on what each entry is, though hledger decides nothing (§1) |
| `hledger_reads_declarations` | hledger's declared accounts, commodities and prices are the model's |
| `hledger_rolls_parents_up` | a parent's balance is the sum of its leaves', as hledger rolls accounts up at depths 1 and 2 (§2.2) |
| `at_cost_everything_sums_to_nought` | at cost, the balances of all accounts sum to nought in every commodity, as hledger values them (§2.3) |
| `hledger_catches_what_check_catches` | a whole statement's balance, an entry's posting or a declaration, tampered with past `check`, is refused by hledger on its own: its reading is a second computation (§6.3) |
| `extremes_survive_hledger` | in a commodity of 0 to 16 places, the greatest and least amounts the bound allows, and a cost at the bound, are taken, read back, and read by hledger as written; one unit more is refused (I-K0, I-K1, I-K3) |
| `any_text_the_rules_allow_survives` | any description and account segment the rules allow, hledger reads exactly as written; any text the configuration and an invoice may hold comes back from `abo.toml` and the `.json` unchanged (I-E0, §2.2) |
| **Operations** | |
| `each_operation_changes_only_its_part` | an operation changes its own part of the state and nothing else, by one; a bill may also link one document, to itself (§4) |
| `balances_move_by_the_new_entry` | an operation moves the balances by the new entry's postings, and by nothing else |
| `inverses_restore` | add and remove, file and unfile, record and unrecord, set and unset: the second gives back the books as they were (§4) |
| `setting_twice_is_setting_once` | `config set` of a key to a value, twice, is once (§4.2) |
| `reverting_twice_restores` | reverting an entry and then its reversal leaves every earlier entry standing as it did, every debt as open, the balances as they were (I-R1, §2.7) |
| `paying_what_is_open_settles` | paying a debt what it has open settles it; paying more is refused, by I-P2 |
| `writing_off_settles` | a write-off leaves nothing open; refused, it names a rule |
| `crediting_cancels` | a credit note cancels its invoice — it does not stand, and nothing is open — and takes the next number of its year |
| `paying_in_parts_settles` | a debt paid in two parts is settled, as paid in one |
| `payments_are_reverted_first` | a debt with a payment standing is not reverted or credited; once its payments are reverted, it is (I-R3) |
| `invoices_are_credited_not_reverted` | an invoice or credit note is not reverted (I-R1) |
| `a_document_follows_its_bill` | a document that names a bill is neither taken out nor filed again; its bill booked, reverted and booked again keeps it (§2.5) |
| `an_account_is_removable_iff_unused`, `a_commodity_is_removable_iff_unused` | a declaration is removed exactly when nothing uses it (§4.1, I-A1) |
| `declaring_under_a_posted_account_is_refused` | no account is declared under one that is posted to (I-A2) |
| `closed_dates_are_refused` | a period closed, an entry or statement dated in it is refused, by N2, and so is closing less, by N6 |
| `issuing_follows_the_configuration` | an invoice is issued with the accounts, terms, VAT method and rate configured on its day, and in another commodity the latest price (§2.5, §4) |
| `a_key_set_reads_back`, `unknown_keys_are_refused` | a key set reads back as set; a key that names nothing is refused (§4.2) |
| `clearing_everything_makes_a_statement_whole` | a statement clearing all that is not yet cleared, at what it sums to, is whole, and asserted for hledger (§6.3) |
| `unrecording_and_recording_again_restores` | the latest statement taken out and recorded again gives the books as they were |
| `the_order_of_entries_does_not_change_balances` | two entries booked in either order give the same balances |
| **What stays** | |
| `entries_never_change` | an entry, once added, is unchanged, and so are its derived postings and number (§2.3, §2.7) |
| `closed_periods_stay_closed` | no operation adds, removes or changes what is on or before `closed_through`, and it does not move back (N2, N6) |
| `documents_keep_their_bill` | a document filed for a bill keeps it while the bill stands (§2.5) |
| `nothing_is_dated_after_today` | nothing added is dated after the day it is added (N1) |
| **What is derived** | |
| `open_debts_are_the_balances` | on every account a debt is on, the balance is what the debts there have open (I-P3) |
| `open_lies_between_nought_and_owed` | what a debt that stands has open is between nought and what it owes, and nought once it does not stand (§2.7) |
| `invoice_numbers_run_without_gaps` | in each year, invoices and credit notes are numbered 1, 2, 3, … in the order of their ids and their dates (§2.7, I-I2) |
| `invoices_post_what_they_say` | an invoice's entry debits the receivable with its total, and credits output VAT with its VAT and income with its net, or income with its total (§2.4) |
| `whole_statements_are_the_balance` | a whole statement states the account's balance on its date (§6.3) |
| `standing_and_paid_by_their_definitions` | `standing` and `paid`, computed from the words of §2.7 alone by recursion, are the model's |
| `names_mean_nothing` | renamed throughout, each segment but the class to a name of its own, the books break the same rules, balance the same, and take or refuse every operation alike, by the same rules: roles come from the configuration, never from names (§2.2) |
| `document_paths_are_distinct`, `documents_alike_are_refused` | no two documents have one path; of two of one issuer and day whose hashes begin alike, the second is refused (I-B1) |
| `rounding_is_half_away_from_nought` | rounding is to the nearest, half away from nought, the same either side of it, to no more places than asked (§2.5) |
| **This document** | |
| `refusals_name_a_rule` | every refusal names a rule this document defines, or `OP` |
| `every_rule_is_checked` | every rule this document defines is checked in the model's code |
| `enough_is_taken` | of the steps 100 plans draw, at least a third are taken: an invariant that refused everything would pass the rest |

The properties that take an operation to the state a plan reaches reach their
assertions in a sixth to a half of their cases, measured; the rest are states
the operation does not apply to, such as crediting an invoice that is paid.

A plan is a list of steps drawn without looking at the books: post, file,
unfile, bill, invoice, credit, pay, write-off, revert, record, unrecord,
config set and unset, the declarations' add and remove, and undo. Each is read
against the state it meets — an index picks among the debts, entries,
documents and accounts there are — so that proptest shrinks a failing plan by
dropping and simplifying steps, and what is left still means something. Values
are mostly what a person would enter, and now and then whatever `arbitrary`
makes of a decimal, a date or a string. Failing plans proptest found are kept
in `proptest-regressions/`, and run first every time. `PROPTEST_CASES` sets how
many plans each property draws; 256 by default, and 48 for hledger's.

What the properties found, each now a rule above:

- hledger takes a total cost's sign from its amount: a cost written with a
  sign was read as the opposite one. Costs are written unsigned (§6.2).
- hledger wants a decimal point in a commodity directive: a commodity of no
  places is declared `commodity 1000. JPY` (§6.4).
- Numbers past an exact decimal's range made the model panic: I-K3.
- Dates outside four-digit years were written as nothing could read: I-D1,
  and years are written with four digits, `0841`.
- A plain entry to the receivable before the first invoice made the balance
  and what is open disagree: N5 became I-P3.
- Reading an edited `bills/index` panicked on a hash not in ASCII: `read` is
  total on any text.
- I-I2 compared an invoice's date with the one numbered just before it, so an
  invoice of the year before, between two of this year's, let them be dated
  out of order: it now compares within the year.
- An invoice's total could pass the bound its lines kept, and so be owed in
  an amount no payment could hold: I-K3 bounds totals too.
- Two documents of one issuer and day whose hashes begin alike had one path:
  I-B1 refuses the second.
- hledger reads every Unicode space in an account's name — a no-break space
  among them — as U+0020: two names abo told apart were one to hledger. A
  segment holds no other space (§2.2).
- abo began an invoice's description with the customer's name, so a customer
  named `(Acme) GmbH` was read by hledger as a code, and refused: the number
  comes first (§6.2). A customer whose first line was blank was refused by the
  description's rule, not the invoice's: I-I1 asks for a name.
- An amount at the bound in a commodity of 17 or 18 places is 29 or 30 digits,
  past what an exact decimal holds: I-K0 allows 16.
- `render` rounded an amount to its commodity's places, so `abo tidy` changed
  an amount with more places than declared, or in a commodity not declared:
  it pads, and never rounds.
- `config set` read every value beginning with `{` as a table, so an address
  beginning with one could not be set: a value is the first reading the key
  takes (§4.2).
- A problem at a file's end — its last newline gone — named a line past it.

The 172 regression tests stay; most become tests of one invariant each, once
the commands are built on the model.
