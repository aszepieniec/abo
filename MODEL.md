# abo — the model of the books

DESIGN.md says how abo is built and why; WORKFLOWS.md what it is for. This
document defines what the books *are*: a set of states, the invariants every
state satisfies, the operations that take one state to the next, and the one
text that represents each state. Where it departs from how abo works today, it
says so.

The definitions are also code: `src/model`, whose types, `check`, `apply`,
`render` and `parse` are the ones below, and whose tests are the claims this
document makes (§11). Where the two differ, the code is wrong or this is, and
a test is what says which.

---

## 1. Definitions

**The model** is a set M of states. A state `m ∈ M` is a value of the type
`Books` below whose invariants (§4) all hold.

**The representation** is a function `render: Books → Files`, from a state
to the contents of the text files in the books' directory: `abo.toml`, the
journal, each invoice's `.json`, `partners.toml`, each agreement's
`contracts/<id>.toml`, `bills/index` and `contracts/index`. It is injective: two
different states render to different files. The image of `render` is the set
of *canonical* files. The binary files — bills' and agreements' documents,
invoices' PDFs —
are not rendered but kept as they came or were made; the state holds the hash
of each (§2.5).

**Reading** is a function `parse: Files → Books ∪ {Problems}`, with

    parse(render(m)) = m        for every m ∈ M.

On files that are not canonical — a hand edit, books written by an older abo —
`parse` returns the state it can read and the list of places where the files
depart from canonical form, each named by file and line. It never refuses to
read the books on that account (§8).

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
invariant to say it does not; a value derived cannot. So an invariant (§4)
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
SHA-256 hash, in lower-case hex; `EntryId` an entry's place in `entries`,
a gap included (§2.3).

### 2.1 The books

```rust
struct Books {
    config:      Config,                                      // abo.toml (§2.8)
    accounts:    Set<Account>,                                // the declared accounts
    commodities: Map<Commodity, Places>,                      // every commodity, with its places
    prices:      Map<(Commodity, Date), Amount>,              // what one unit was worth, in another
    entries:     Vec<Entry>,                                  // in the order added; never changed
    statements:  Map<(Account, Commodity, Date), Statement>,  // from the bank
    documents:   Map<Hash, Document>,                         // bills' and agreements' documents, by their hash
    partners:    Map<Id, Partner>,                            // whom invoices go to and bills come from (§2.9)
    agreements:  Map<Id, Agreement>,                          // contracts with partners, in versions (§2.9)
}
```

A map's key is what identifies its value, so what would otherwise be a rule —
one price per commodity and date, one statement per account, commodity and
date, no document filed twice — is how the state is built, and needs no
invariant.

### 2.2 Accounts, commodities and prices

`Account` is a path of segments joined by `:`: `Assets:Bank:PostFinance`. A
segment is a non-empty text that holds no `:`, which separates segments, and
no two spaces in a row, which end the name — those from hledger's grammar —
and whose every character hledger reads as written where it stands
(§2.3, *Which characters*). `Commodity` is three or more capital letters, A to
Z: EUR, CHF, BTC. `Places` is the digits after the point, 0 to 8:
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

Every entry has an **id**: its place in `entries`, counting from 1. A place may
hold a **gap** — `Kind::Gone` — an id given to an entry that undo took back.
The **next id** is one more than the highest id ever given: `entries` holds
every id given, gaps included, so it is `len(entries) + 1`. Adding an entry
takes it; undo takes back the entry but not its id (§5.3), so an id, once
seen in a commit, the log or a message, names that entry alone, or a gap,
ever after. Nothing names a gap, and a gap has no postings. An entry names another — a payment the debt it pays, a reversal what
it reverses, a credit note the invoice it cancels — by its id, and so does a
document or a statement; the id does not move when other entries are booked
around it. Once added, an entry never changes.

```rust
struct Entry {
    date:        Date,
    description: Text,   // not empty; every character one hledger reads as written
    kind:        Kind,
}

struct Posting {
    account: Account,
    amount:  Amount,
    cost:    Option<Amount>,   // what the whole amount was exchanged for
}

struct Amount { quantity: Decimal, commodity: Commodity }
```

A description is what the date line holds after the date.

**Which characters.** What hledger reads back as written is not guessed from
its grammar: `hledger_reads_these_characters` asks hledger of every character
`hidden` lets through, at the start, in the middle and at the end of a
description and of an account segment, each in an entry of its own, and
`hledger_changes` is the table it found. With hledger 1.50.3:

| | Start | Middle | End |
|---|---|---|---|
| Description | a space, `!`, `(`, `*`, `;` | `;` | a space, `;` |
| Segment | another space | another space | a space |

where *a space* is U+0020 or any other Unicode space — the no-break space,
U+2000 to U+200A, U+3000 and the rest — and *another space* any but U+0020,
which hledger reads in an account's name as U+0020. In a description, `;`
begins a comment, and `!`, `*` and `(` at the start a status or a code. The
test is slow, and is run when hledger is upgraded; it fails where the table is
no longer what hledger does.

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
    /// Booked by hand: anything that balances. When made for a period of an
    /// agreement's schedule, the agreement and the period (§2.9).
    Plain(Vec<Posting>, Option<(Id, Period)>),
    /// A supplier's bill. A debt. What the supplier says of it besides.
    Bill(Vec<Posting>, BillMore),
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

A bill may say, besides its postings, what the supplier's own number for it
is and when it is due; either, both or neither, as the bill gives them:

```rust
struct BillMore {
    number:  Text,          // as the bill prints it; empty where it has none
    due:     Option<Date>,  // on or after the bill's date
    partner: Option<Id>,    // who sent it (§2.9)
}
```

They are inputs, said by whoever books the bill. The partner is said when
the bill is booked, or never: the entry does not change after. A bill booked
without one, or with the wrong one, is reverted and booked again. A bill with something open
past its due date is overdue, as an invoice is.

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
its hash; **an agreement's**, a contract or an amendment as signed, likewise
under `contracts/` (§2.9):

```rust
struct Document {
    date: Date,
    name: Text,              // the issuer, as the path says it: a-z, 0-9 and '-'
    ext:  Text,              // pdf, jpg: a-z and 0-9
    of:   Option<Of>,        // what it is the document of; none: a bill not booked yet
}

enum Of {
    Bill(EntryId),           // the bill it is the document of, once booked
    Version(Id, usize),      // an agreement, and the place of the version it is of
}
```

Its path follows from these and its hash:
`bills/<year>/<date>-<name>-<first 8 digits of the hash>.<ext>`, or, of a
version of agreement `<id>`, `contracts/<id>/<date>-<name>-<first 8 digits
of the hash>.<ext>`. The hash is a check: the file at that path has it, or
`parse` reports it (§8). One map holds both, so a file is filed once, as a
bill's document or as an agreement's, not as both.

The document names its bill, not the bill its document: a booking and its
document come in either order, and the one that comes second records the link
— on the document — so that no entry changes, as a statement names the
postings it clears. What the set of documents says:

- **Filed, not booked yet:** a document whose `of` is none, or names a bill
  that does not stand. It may be taken out again (`unfile`), or booked; one
  that names a standing bill may not be taken out.
- **Booked, document missing:** a `Bill` entry no document names. An ordinary
  state, not an error: filing the document later, for that booking, sets its
  `of`.
- **Booked again:** a document whose bill is reversed may name another, so
  that a bill booked wrongly, reverted, and booked again keeps its document.
- **Duplicates:** filing a document whose hash is filed already is refused.

An agreement's document is filed with the version it is of, in one
operation, and so is never without one: a document without one is a bill
not booked yet, and the Bills page lists it as one. It names its version and
the version does not name it, so a contract's scan is filed for version 1
after version 3 is added, and no version changes. It is not taken out: the
agreement is kept as a record, and its documents with it. Filed by mistake,
it is taken back by undo while nothing has happened since.

**An invoice's content** is what it says. Its amounts are not in it: they
follow.

```rust
struct Content {
    to:        Option<To>,            // the partner, or the agreement, it is issued under (§2.9)
    period:    Option<Period>,        // what it is for, when it is for a period
    customer:  Lines,                 // name, then address
    commodity: Commodity,
    home:      Option<Price>,         // in another commodity: what one unit was worth in the company's
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

/// As configured when it was issued; income as the operation named it, the
/// configured one unless it named another (an export's, a reverse charge's).
struct InvoiceAccounts { receivable: Account, income: Account, output: Option<Account> }

enum InvoiceVat {
    /// Issued while the company was not VAT registered: it says nothing of VAT.
    NotRegistered,
    /// Registered, charging none — an export, an exempt supply — and why.
    Exempt { reason: Text },
    /// Charged at `rate` percent.
    Charged { rate: Decimal },
}

struct Price { date: Date, price: Amount }

enum To { Partner(Id), Agreement(Id) }

struct Period { from: Date, to: Date }   // both days included
```

`to` names the partner the invoice is issued to, or, for a period of an
agreement, the agreement, whose schedule in force on the period's first day
names the party it is issued to: one or the other, so the partner is held
once. `customer` and `customer_tax_id` are what the
invoice printed, copied from the partner when it was issued and kept as
printed: the partner's address may change after, and the invoice does not.
An invoice issued before partners were, or to someone who is not one, has
no `to`. `period` is what the invoice is for — the period of an agreement it
bills, or of any other supply — and is printed on it, as an invoice must
say when its supply was made.

The amounts, each rounded half away from nought to the commodity's places:

    amount(line) = quantity × unit_price
    net          = Σ amount(line)
    vat          = net × rate / 100    when Charged, else nought
    total        = net + vat

In another commodity than the company's, net and VAT are also taken at the
home price, each rounded to the company currency's places:

    net_home     = net × home.price
    vat_home     = vat × home.price

and the entry is booked in the company's currency (I-E4): the receivable in
the invoice's commodity at a cost of `net_home + vat_home`, income and output
VAT at `net_home` and `vat_home`. The PDF states the VAT at `vat_home` too.

Everything an invoice depends on that can change later — the configured
accounts, the VAT rate, the payment options, a price — it holds as it was when
issued. Those are inputs to it, not copies: the configuration says what holds
now, the invoice what held then. A commodity's places do not change while
anything is in it (§5.1), so the amounts, computed again, are always the ones
printed.

**An invoice's PDF** is made once, when the invoice is issued, from its content
and the company's template, and kept as made; a credit note's from the
invoice's content, negated, and its note. It is not made again: the template
is the company's to change, and an invoice already sent must not change with
it. Its hash is a check, so that a PDF changed or lost since is a problem
`parse` reports (§8).

Every invoice in the books is one abo issued, so every invoice has both. An
invoice issued elsewhere — before abo, or by other software — is not an
invoice to the model; should books ever need to hold one, that is the time to
model it.

**Other supporting documents** — a receipt, a bank's confirmation — are left
out of the model. Should they be wanted, they would be kept as bills' and
agreements' documents are.

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
| `whole(s)` | every posting to the statement's account, in its commodity, dated on or before it, is cleared by it or an earlier statement: its balance is then the account's, and hledger checks it too (§7.3) |
| `partner(e)` | of a bill, its `partner`; of an invoice, the partner its `to` names, or, as of a plain entry for a period, the party its agreement's schedule in force on the period names; of a credit note, a payment or a reversal, that of the entry it names; else none |
| `customer(p)`, `supplier(p)` | an invoice, a bill, has `partner` p |
| `agreements(p)` | the agreements `p` is a party to |
| `active(a)` | `a`'s status is `Active` |
| `last_day(a)` | the earlier of the `until` of `a`'s version in force and the day its status ends it, where either is |
| `in_force(a, d)` | the last added of `a`'s versions whose `from` is on or before `d` (§2.9) |
| `periods(a)` | §2.9 |
| `done(a, P)` | a standing invoice or entry names `a` and has period `P` |
| `due(a, d)` | the periods of `a` that can be done by `d`, and are not (§2.9) |
| `owed_to(p, c)`, `owed_by(p, c)` | the sum of `open(d)` over the standing invoices, the standing bills, `d` in commodity `c` with `partner(d) = p` |

`reversed` and `standing` are defined by each other; since a reverser has a
greater id than what it reverses (I-E3), they are defined by induction on the
id, from the last entry down.

An invoice's number is its place, not a stored value: the numbers of a year
run 1, 2, 3, … with no gap and no repeat because they are counted, not
assigned. Nothing removes one: undo does not take back an invoice or a
credit note issued (§5.3).

Because a payment names one debt, `paid` and `open` are always known. The `?`
of today's Bills page cannot occur.

### 2.8 Configuration

`Config` is `abo.toml`: the company's name, address, tax number and currency,
and the date the books are closed through; the VAT method and its accounts;
the receivable, income, terms and VAT rate invoices are issued with; the
payment options, by id; and the tax return's boxes, by code. Its invariants
are in §4.8. It is part of the state, so changing it is an operation like any
other (§5.2).

### 2.9 Partners and agreements

*In the model since 2026-11-04: the state, its rules, guarantees and
operations. The commands and pages of §5.5 are not built yet.*

A **partner** is someone the company invoices or is billed by, pays or is
paid by, held so that what an invoice prints of them is said once:

```rust
struct Partner {
    lines:    Lines,   // their name, then their address, if given, as an invoice prints them
    tax_id:   Text,    // as an invoice prints it; empty where none is given
    employee: bool,    // one the company employs
}
```

A partner's name is all that is asked of them: an address or a tax number
is given where something prints it, and an employee's, which nothing
prints, may be left out. A partner has no role but one: an **employee** is
a partner the company employs, listed apart. Whether a partner is a
customer, a supplier or both is what names them: an invoice, a bill (§2.7).
A partner's **agreements** are those they are a party to, none or more: the
agreement holds its parties, and the partner's list is derived from them,
so it is held once.

An employee is paid under an agreement — their contract — whose schedule
pays them. One who is not, while employed, is shown so on the Partners
page; it is not a rule, since an employee is added before the contract that
names them.

An `Id` is a-z, 0-9 and `-`, as a document's name: it is written in the
journal as a tag's value, which hledger ends at a comma, and typed on the
command line. An id is not changed, since invoices and bills name it and
they do not change. A partner's details are changed at any time; what an
invoice printed of them is in its content, and does not change with them. A
partner is removed only while nothing names them: an id removed and added
again would otherwise name another person on the first one's invoices and
bills.

An **agreement** is a contract between the company and one or more
partners, held as its parties, its status and the list of its versions. A
change to its terms adds a version; the versions before stay as they were,
the record of what was agreed when.

```rust
struct Agreement {
    parties:  Vec<Id>,           // the partners party to it, one or more, as given
    status:   Status,
    versions: Vec<Version>,      // in the order added; never changed
}

enum Status {
    Active,
    Terminated { on: Date },     // its last day
    /// Replaced by another agreement, which holds what it agreed from the day after.
    Subsumed { by: Id, on: Date },
}

struct Version {
    from:     Date,              // when it takes effect: any day
    until:    Option<Date>,      // the agreement's last day, if it ends
    schedule: Option<Schedule>,  // what the company invoices, pays or receives under it, and when
}

struct Schedule {
    party:  Id,                  // whom: one of the agreement's parties
    does:   Does,
    every:  Months,              // the length of each period, 1 or more
    billed: Billed,
    draft:  Fields,              // the form's fields, as each invoice or entry starts from them
}

enum Does {
    Invoice,                     // an invoice to the party, from the invoice form
    Pay,                         // an entry paying them — a salary, a rent — from the entry form
    Receive,                     // an entry of what they pay without an invoice, likewise
}

enum Billed { InAdvance, InArrears }
```

A version's number is its place in the list, from 1: `acme-hosting/2`. The
parties are the agreement's, not a version's: a contract with other partners
is another agreement. A version without a schedule is a contract on record —
a lease paid on each bill, a supplier's terms, a contract invoiced by hand; a
version with one is also a schedule of invoices or entries, with the party
it names. An agreement posts nothing itself: what is posted under it is an
invoice or an entry, made by someone.

**Status.** An agreement is active until it is terminated, or subsumed by
another — a framework contract replacing the separate ones it gathers, a
renegotiation that starts afresh — on a day, its last. One not active stays
as it is: no version is added to it, and its status does not change but by
undo. What was done under it stays done, and it is kept as a record, with
its documents. `until` is what a version says of when the contract ends;
the status is what became of it. **The agreement's last day** is the
earlier of the two, where either is.

**The version in force** on a day is the last added of those whose `from` is
on or before it. Two versions may have the same `from`: the later is in
force, and the earlier stays as the record of what it replaced — a mistake
corrected, or terms changed again before they took effect. To end an
agreement as its terms say is to add a version with an `until`; to end it
otherwise, to terminate it.

**Periods.** Of a version with a schedule, block k, counting from 0, runs
from the 1st of the month `k × every` months after the month of `from`, to
the day before block k + 1: whole months, each beginning on a date that
exists. The version's periods are its blocks cut to the days it is in force
— from its `from` to the day before the next version's, and to the
agreement's last day — those left with no day dropped. So a contract that
begins on the 15th has a first period from the 15th to the end of its
block, one that ends on the 10th a last period to the 10th, and a version
that takes effect mid-period ends the period before it there; the periods
between are whole. The draft is a whole period's: what a short one owes is
said by hand, on its invoice or entry, and the period written on it says
which days it is for.

A period is **done** while a standing invoice or entry names the agreement
and that period: one credited or reverted is not, and is done again. A
period can be done from its first day when billed in advance, from the day
after its last when billed in arrears. **Due**, on a day, is every period of
the agreement that can be done by then and is not — to invoice, to pay or
to receive, as the version in force on it says, with the party it names. An
agreement ended keeps its last periods due until they are done: last
month's salary is paid after the contract ends.
abo does nothing of its own accord. Invoice numbers follow dates (I-I2) and
nothing is dated after the day it is added (N1), so a period missed is done
on the day it is noticed, and the period written on it says what it is
for. The draft fills in the form; the person doing it says what changes —
this month's hours, this month's payroll — and makes it.

The draft is the form's fields, not an invoice or an entry. It is checked
as any text is (I-C1), and for nothing it names: an account moved or a
payment option removed since leaves it naming one that is not there, and
making one from it is refused as the form refuses one typed. It is
corrected in the form, and in a new version.

**What was done stays as done.** The version an invoice or entry was made
under is derived — the version in force on its period's first day — so it
must stay the one in force: a version is not added that takes effect on or
before the last day of a period done (N10). A change of price, of terms or
of period begins after what has been done.

An agreement's documents — the contract, each amendment — are kept as bills'
documents are, each naming its version (§2.5).

---

## 3. Guarantees

A **guarantee** is a property of the books that something reading them relies
on. The readers are hledger; the files on disk and whoever reads or edits
them; the record of what each debt owes; an invoice and the customer who
received it; the bank statement; the tax return and the periods filed with it; a bill's
document; a partner, an employee, and their agreements; and whoever runs a command. The list below is closed: it is what
the books promise, and nothing else is promised.

Three rules tie it to the rest of this document:

- Every rule of §4 is there because a guarantee needs it, and the table says
  which. A rule no guarantee needs is removed.
- Every guarantee has the properties of §11 that test it, and the table names
  them; `every_guarantee_is_tested` checks that each property named exists
  and that every property is named. A property no guarantee needs is removed.
- A new property is written when a new reader is added, with its guarantee —
  not otherwise.

A property that fails then means that the rules do not imply a guarantee, or
that the code does not keep the rules; it never means that a guarantee was
missing, because the list says what is guaranteed.

The guarantees are of the books: broken, the books contradict themselves or
lose something. The last table holds what is not: the messages abo gives, and
the checks that this document and its tests stay in step with the code. Their
tests stay, and a failure there is a defect in a message or in this document,
not in the books.

### hledger

| | Guarantee | Rules | Properties |
|---|---|---|---|
| G-H1 | hledger reads the journal without error, checking its accounts, commodities, balance, date order and balance assertions | I-A1, I-K0, I-K1, I-D1, I-E0, I-E1, I-E2, I-S3, §7 | `hledger_agrees` |
| G-H2 | hledger reads every entry, account, commodity and price exactly as abo holds it: date, description, tags, account, amount, cost | I-A0, I-K0, I-K3, I-D1, I-E0, §7 | `hledger_reads_entries_as_written`, `hledger_reads_declarations`, `hledger_reads_these_characters`, `any_text_the_rules_allow_survives`, `extremes_survive_hledger` |
| G-H3 | hledger's balance of every account, in every commodity, through any date, at any depth and at cost, is the model's | I-A2, I-E1, I-E2 | `hledger_agrees`, `hledger_agrees_through_any_date`, `hledger_rolls_parents_up`, `at_cost_everything_sums_to_nought` |
| G-H4 | hledger checks every whole statement, every entry's balance and every account's declaration on its own, so that a fault in `check` shows as a disagreement | I-S3, §7.3 | `hledger_catches_what_check_catches` |
| G-H5 | The balance sheet comes out nil, whatever the books hold: an exchange at a cost is equity's conversion | I-E1, I-E2 | `the_balance_sheet_comes_out_nil` |

### The files

| | Guarantee | Rules | Properties |
|---|---|---|---|
| G-F1 | The files say the state and nothing else: `parse(render(m)) = m`, so each state has one text | §7 | `files_say_the_state` |
| G-F2 | An edit to the files is never read as the same state with no problem | §7, §8 | `edits_are_noticed` |
| G-F3 | Reading never fails, whatever the text: it gives a state, or problems | §8 | `edits_are_noticed` |
| G-F4 | `abo tidy`, which is `render(parse(f))`, gives text read as the same state, and tidying it again changes nothing | §7, §8 | `tidying_is_idempotent` |
| G-F5 | Books holding a problem — a hand edit, a file changed — are read; while they break a rule, the only change made is one that mends one and brings none; `abo check` fails exactly while one is there, and `abo tidy` mends what is only not as abo writes it | §8 | `hand_edits_are_checked_and_tidied` |

### Debts

What is owed, debt by debt: read by the Bills and Invoices pages, by `pay`
and `write_off`, and by whoever reconciles the receivable and the payable
with the ledger. Broken, the ledger and the debts in it disagree.

| | Guarantee | Rules | Properties |
|---|---|---|---|
| G-D1 | For every debt, what it owes, what is paid and what is open are defined; what is open is between nought and what it owes, and nought once it is cancelled | I-B2, I-P1, I-P2, I-R1, I-R3, I-E3 | `open_lies_between_nought_and_owed`, `standing_and_paid_by_their_definitions` |
| G-D2 | On every account a debt is on, the balance is what the debts there have open | I-P3 | `open_debts_are_the_balances` |
| G-D3 | A debt is settled by payments and write-offs, through money accounts, and cancelled by its reversal or credit note, in no other way; what pays it is reverted before it is cancelled | I-P1, I-P2, I-R1, I-R3, N4 | `paying_what_is_open_settles`, `paying_in_parts_settles`, `writing_off_settles`, `crediting_cancels`, `payments_are_reverted_first`, `reverting_twice_restores`, `payments_move_through_money_accounts` |

### An invoice, and its customer

| | Guarantee | Rules | Properties |
|---|---|---|---|
| G-I1 | An invoice's entry posts what the invoice says, its amounts rounded commercially | §2.4, §2.5 | `invoices_post_what_they_say`, `rounding_is_half_away_from_nought` |
| G-I2 | Invoice numbers run 1, 2, 3, … within each year, in date order, without gap or repeat | I-I2 | `invoice_numbers_run_without_gaps` |
| G-I3 | An invoice, once issued, never changes — its number, amounts and postings — whatever is configured, declared or booked after it; it is corrected by a credit note | I-K0, I-R1, §5.1 | `entries_never_change`, `invoices_are_credited_not_reverted` |
| G-I4 | An invoice is issued as the configuration and the declarations stand on its day | I-I1, I-C1, I-C2, I-K2, N3 | `issuing_follows_the_configuration` |

### The bank statement

| | Guarantee | Rules | Properties |
|---|---|---|---|
| G-S1 | A posting is cleared by one statement at most; a statement's balance is what it and the account's earlier ones clear, and a whole one is the account's balance | I-S2, I-S3 | `whole_statements_are_the_balance`, `clearing_everything_makes_a_statement_whole`, `unrecording_and_recording_again_restores` |
| G-S2 | A statement once recorded is not undermined: nothing is added before it that it would have shown, so a whole one stays whole | N7 | `statements_stay_whole` |
| G-S3 | What a statement shows is not reversed: an entry whose posting it clears is not reverted, so the books hold no movement the bank never made | N9 | `cleared_entries_are_not_reverted` |

### The tax return, and periods filed

| | Guarantee | Rules | Properties |
|---|---|---|---|
| G-P1 | Nothing dated on or before `closed_through` changes, and it never moves back | N2, N6 | `closed_periods_stay_closed`, `closed_dates_are_refused` |
| G-P2 | Nothing is dated after the day it is added | N1 | `nothing_is_dated_after_today` |
| G-P4 | Income and expenses are in the company's currency, so profit and loss, and the balance sheet's unclosed profit, are one amount each | I-E4 | `income_and_expenses_are_in_the_company_currency` |
| G-P3 | VAT accounts are posted to only as the VAT method in force allows: input VAT by bills, output VAT by invoices, a flat rate's account by neither; each also by plain entries, and by reversals and credit notes, which undo what was booked under the method of its day; none by a payment or a write-off | N3, I-C3 | `vat_accounts_move_as_the_method_says` |

### A bill's document

| | Guarantee | Rules | Properties |
|---|---|---|---|
| G-B1 | Each document has a path of its own and is filed once; it keeps its bill while the bill stands, and follows it when it is booked again | I-B1 | `document_paths_are_distinct`, `documents_alike_are_refused`, `documents_keep_their_bill`, `a_document_follows_its_bill` |
| G-B2 | A bill is not booked twice by mistake: one owing what a standing bill owes, within a week of it or under the same number of the supplier's, is booked only when said to be another; one under a standing bill's number of the supplier's, owing the same, never; one from a partner under a number of theirs a standing bill of theirs has, never, whatever either owes | N8 | `bills_alike_are_refused_unless_another` |
| G-B3 | What a bill says of itself besides its postings, the supplier's number, when it is due and who sent it, is read back as said, by abo and by hledger alike, and a due date is never before the bill | I-B3 | `files_say_the_state`, `hledger_reads_entries_as_written` |

### A partner, and their agreements

| | Guarantee | Rules | Properties |
|---|---|---|---|
| G-T1 | Whatever names a partner or an agreement — an invoice, a bill, an entry, an agreement, a document — names one there is; one is removed exactly when nothing names it | I-T1, I-T2 | `a_partner_is_removable_iff_unnamed`, `an_agreement_is_removable_iff_unnamed` |
| G-T2 | An agreement is kept as a record: a version once added never changes, and is not removed, nor is a document of one, but by undo; one terminated or subsumed stays so, takes no new version and has no period past its last day, and no agreement is subsumed, through others, by itself | I-T3, I-T5, N11 | `versions_are_kept`, `ended_agreements_stay_ended` |
| G-T3 | A period of an agreement is done once — invoiced, paid or received — while its invoice or entry stands, and the version in force on it stays the one it was made under | I-T4, N10 | `periods_are_done_once`, `what_is_done_keeps_its_version` |
| G-T4 | What a partner owes, and is owed, in each commodity, is what their debts have open | I-T2, I-P1 | `partners_owe_what_their_debts_have_open` |

### Whoever runs a command

| | Guarantee | Rules | Properties |
|---|---|---|---|
| G-O1 | An operation changes what it names, and nothing else; an entry once added never changes; the opposite of an operation gives back the books as they were | §5 | `each_operation_changes_only_its_part`, `balances_move_by_the_new_entry`, `entries_never_change`, `inverses_restore`, `setting_twice_is_setting_once`, `reverting_twice_restores` |
| G-O2 | Balances depend on the entries alone: not on the order they are booked in, nor on the names of the accounts — roles come from the configuration | I-C3, I-A0 | `the_order_of_entries_does_not_change_balances`, `names_mean_nothing` |
| G-O3 | A declaration is removed exactly when nothing uses it, and nothing is declared under an account posted to | I-A1, I-A2 | `an_account_is_removable_iff_unused`, `a_commodity_is_removable_iff_unused`, `declaring_under_a_posted_account_is_refused` |
| G-O4 | A configuration key reads back as set; a key that names nothing is refused | I-C1, I-C2, I-C3, §5.2 | `a_key_set_reads_back`, `unknown_keys_are_refused` |
| G-O5 | Every command does what its operation does in the model: taken exactly when the model takes it; then the files say the model's state, with every document and PDF beside them; refused, with the rules the model names, in JSON and in words, and nothing written; undo as §5.3. What the commands show is the model's | §5 | `the_cli_is_the_model`, `refusals_in_words_name_their_rules`, `what_abo_shows_is_the_models`, `lists_show_what_the_books_hold`, `reports_read_the_books_as_the_model_does`, `a_key_set_reads_back_through_the_cli`, `company_changes_only_what_it_is_told`, `payment_options_follow_add_edit_remove`, `pay_takes_what_is_open_from_money`, `record_all_clears_everything`, `a_document_filed_for_a_bill_names_it` |
| G-O6 | Changes sent at once are each made, one after another | §5 | `changes_at_once_are_each_made` |
| G-O7 | `abo init`, and the form that asks its questions in the browser, open books that read with no problem, configured as told; or refuse and leave nothing behind | §2.8 | `init_opens_valid_books_or_nothing`, `a_new_company_opens_like_init` |
| G-O8 | The log holds each change as the command that makes it again, whoever made it, a terminal or a form: every part of the operation written out, so that run on the books as they were, on any later day, it makes the same change. One function writes an operation as its command, another as its commit's subject | §5 | `the_log_makes_each_change_again`, `the_web_is_the_model` |
| G-O9 | A report changes nothing in the books, wherever it is told to write: a PDF, or hledger's output file, goes outside them or is refused | §5 | `a_report_changes_nothing_in_the_books` |
| G-O11 | Every amount in a command's JSON is `{quantity, commodity}`: the number to its commodity's places, the commodity apart, and nothing of how to write it, which is the reader's to choose; what cannot be read as typed ends abo with 2, as a usage error | §5 | `amounts_in_json_are_quantity_and_commodity`, `an_unreadable_date_exits_2` |
| G-O12 | An id once given names that entry, or a gap, ever after: no operation and no undo gives it again; an invoice or credit note issued is not undone | §2.3, §5.3 | `ids_are_never_given_twice`, `issued_invoices_are_not_undone` |
| G-O10 | A refusal says what is wrong with what the operation was given, of the books before it: it names no entry that is not there yet, an invoice or a credit note by its number and never by its id, and quotes nothing as Rust's debug format does | §5 | `refusals_speak_of_the_books_before` |

### Whoever uses the pages

The web pages read the books through the store and change them by the same
operations, each logged as the command that makes the same change.

| | Guarantee | Rules | Properties |
|---|---|---|---|
| G-W1 | A form does what its operation does in the model: taken exactly when the model takes it, and then the files say the model's state; refused, the page names the model's rules, and nothing changes | §5 | `the_web_is_the_model` |
| G-W2 | Every page answers, whatever the books hold: none fails | §5 | `every_page_answers` |
| G-W3 | A form sent twice is done once | §5 | `a_form_sent_twice_changes_once` |
| G-W4 | A post from another site changes nothing | §5 | `cross_site_posts_are_refused` |

### Messages, and checks of this document

Not guarantees of the books: broken, a message is less use, or this document
and the code have drifted apart; the books are as they were.

| | What | Rules | Tests |
|---|---|---|---|
| M-1 | Each problem `read` reports names a line of a file that is there | §8 | `problems_name_lines_that_are_there` |
| M-2 | Every refusal names a rule this document defines | §4 | `refusals_name_a_rule` |
| M-3 | Every rule this document defines is checked in the model's code | §4 | `every_rule_is_checked` |
| M-4 | Every test a row of §3 names exists, and every test is named by a row | §3 | `every_guarantee_is_tested` |
| M-5 | Refusing is not all the rules do: of the steps plans draw, a third or more are taken | §4 | `enough_is_taken` |
| M-6 | Whatever abo is given, it answers: done, refused in words, or a usage message — never a panic | §5 | `no_argument_makes_abo_panic` |

### What the list leaves out

- **Five tests served no guarantee:** `rendered_text_is_tidy`,
  `amounts_are_written_to_their_places`, `the_journal_is_in_order`,
  `the_files_are_exactly_the_states` and `canonical_text` tested §7's
  description of the text, which no reader relies on beyond G-F1 and G-H1.
  They are removed.
- Every number the books hold or compute is exact — no overflow, and no
  rounding but where §2.5 says — is not a guarantee of its own: it is what
  G-H2, G-I1 and G-I3 need, and I-K0, I-K3 and I-D1 are its rules.

---

## 4. Invariants

The invariants and conditions below are the rules that imply the guarantees
of §3; each is there because a guarantee needs it, and §3 says which. Each is
checked in one place, `check`, on the state an operation produces.

Each is numbered so that tests and refusals can name it. An invariant is a
property of the state alone, and says that an input is well formed or that a
check agrees (§1). Rules that also need the day a change is made, or today's
configuration, are conditions on the change, in §4.9. Numbers once used are
not reused: those missing below were rules about a value now derived.

### 4.1 Accounts and commodities

- **I-A0** Every declared account's segments are well formed, and its first
  names a class.
- **I-A1** Every account and every commodity that an entry, a statement, a
  price, an invoice or the configuration names is declared.
- **I-A2** Postings go to leaves alone: every account an entry posts to, a
  statement is of, or the configuration names for postings is a `leaf`.
  Declaring an account under one of those breaks it, and is refused; to split
  an account, its balance is moved by an entry to new accounts beside it.
- **I-K0** A commodity is three or more capital letters, A to Z, with 0 to 8
  places — BTC's eight. With the 12 digits I-K3 allows before the point, an
  amount has 20 digits at most, of the 28 an exact decimal holds: a sum of
  fewer than 10^8 amounts then has 28 at most, and is exact.
- **I-K1** No amount, cost, price or statement balance has more digits after
  the point than its commodity's places.
- **I-K2** A price is above nought, and in another commodity than its own.
- **I-K3** Every number the state holds — an amount, a cost, a price, a
  statement's balance, an invoice line's quantity and unit price — is below
  10^12 in magnitude, and every VAT rate is 0 to 100 percent. Within these
  and I-K0, every sum the books take is exact, and every product an invoice
  takes fits.
  Checked first, and alone: the rest computes with these numbers.
- **I-D1** Every date is in the years 1 to 9999, which the files write as four
  digits and hledger reads. Checked first, and alone, with I-K3.

### 4.2 Entries

- **I-E0** Every description is not empty, and every character in it one
  hledger reads as written where it stands (§2.3).
- **I-E1** Every entry has two postings or more, and balances: for each
  commodity, the amounts of its postings without a cost and the costs of those
  with one sum to nought. This is how hledger checks it.
- **I-E2** A posting's cost is in another commodity than its amount, and both
  are above nought or both below.
- **I-E3** An entry that names another names one with a smaller id, and is
  dated on or after it.
- **I-E4** Every posting to an income or an expense account is in the
  company's currency. What is owed, paid or held in another commodity is in
  it, at its cost in the company's currency (I-E1 then balances it there), so
  that a profit is one amount, and a report in the company's currency needs
  no price.

### 4.3 Bills

- **I-B1** A document's name and extension are well formed, and no two
  documents have one path — which takes two of one issuer and day whose hashes
  share their first 32 bits. Its `of`, when a bill, names a `Bill` entry
  dated as the document, and no two documents name the same bill; when a
  version, one the agreement has (I-T2).
- **I-B2** A bill's booking credits exactly one liability account.
- **I-B3** A bill's number of the supplier's is one line, trimmed, with nothing
  hidden and no comma — hledger ends a tag's value at a comma — and its due
  date, if it has one, is on or after its own.

### 4.4 Invoices

- **I-I1** An invoice has a line or more, each saying what it is for, a total
  above nought, a customer named on the first line,
  and a due date on or after its own; its period, if it has one, ends on
  or after the day it begins. It books output VAT only when it charges
  VAT, and VAT it charges comes to more than nought once rounded to the
  commodity's places; its receivable is an asset, its income an income account,
  its output VAT a liability. An exempt invoice says why. Its total does not come to nought at its home price. A home price is above nought and in
  another commodity. A credit note's `of` is an invoice.
- **I-I2** Within a year, invoices and credit notes are dated in the order of
  their ids, so their numbers follow their dates.

### 4.5 Payments

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
  first invoice on. The property test of §11 found it.

### 4.6 Reversals

- **I-R1** An entry is reversed by at most one standing entry. An invoice is
  reversed by a credit note alone — it is corrected by a credit note, DESIGN
  M3 — and a credit note by nothing.
- **I-R3** An entry is not reversed while another standing entry, other than
  its reversal, names it. So a bill's booking is not reverted, nor an invoice
  credited, while a payment of it stands: the payment is reverted first.

### 4.7 Statements

- **I-S2** A statement clears postings that exist, to its account, in its
  commodity, dated on or before it, and cleared by no other statement.
- **I-S3** A statement's balance is the sum of the postings it and the
  account's earlier statements, in its commodity, clear.

### 4.8 Configuration

- **I-C1** The company has a name; no text in the configuration holds a hidden
  character; no VAT box's query begins with `-`, as an hledger option does.
- **I-C2** Payment option ids are well formed; each option has what its kind
  needs (DESIGN, A4), and nothing another kind has — a cash option no IBAN — and an IBAN or BIC that is valid, in its canonical form,
  if given.
- **I-C3** Every account the configuration names is of its kind: money arrives
  in an asset, the receivable is an asset, income is income, input VAT is an
  asset, output and flat-rate VAT are liabilities. A flat rate is above
  nought; terms and the VAT rate are not below it.

### 4.9 Conditions on a change

These hold of each change when it is made, not of the state: they need the
day it is made, or the configuration in force then, and the state remembers
neither for an entry made long ago.

- **N1** No entry, statement, document or price is dated after the day it is
  added.
- **N2** On or before `closed_through`, no entry is added, no statement
  recorded or removed, no bill's document filed or taken out, and no price
  added or removed. An agreement's document changes no figure of a period,
  so a contract signed in a period closed is filed when it is scanned.
- **N3** VAT accounts are posted to only as the VAT method allows (DESIGN §6):
  a bill posts nothing to output VAT, nor, under a flat rate, to the flat
  rate's account; a payment or a write-off posts nothing to any VAT account —
  the VAT was booked with the debt, and writing a bill off to input VAT would
  change what is reclaimed. An invoice books output VAT as the method in force
  says, which `invoice` does by construction.
- **N4** `pay` moves the money through a money account: an asset, not the
  receivable, not input VAT. `write_off` moves none: it writes an invoice off
  to an expense account, and a bill to an income account, or against an
  expense account the bill posts to, taking back the cost it was booked to.
- **N5** is now I-P3.
- **N6** `closed_through` only moves forward: a period closed stays closed.
- **N7** No entry is added with a posting to an account, in a commodity,
  dated on or before a statement of that account in that commodity. What a
  statement shows stays shown: a whole statement stays whole, and its
  assertion stays written.
- **N9** An entry is not reverted while a statement clears a posting of it:
  the statement shows that the money moved as booked, so the opposite would
  book a movement that never was. What was wrong is corrected on the other
  side, by an entry of its own — an expense moved to the right period — or
  the statement is taken out first, if it was wrong.
- **N8** A bill is not added that owes what a standing bill owes — the same
  amount, in the same commodity, on the same account — dated within 7 days
  of it either side, unless the operation says it is another (`again`). How
  either is split among expenses does not tell two bills apart, nor a day or
  two between the date on it and the day it came: what is owed does. A bill
  that comes monthly, for the same amount, is a month apart. Nor is a bill
  added that owes what a standing bill owes under the same number of the
  supplier's, whatever the dates of the two, and whether or not the
  operation says it is another: a supplier numbers each bill once, so it is
  that bill. Where both bills name the same partner, the number alone says
  so, whatever either owes; where they name two, their numbers say nothing
  of each other.
- **N10** No version is added to an agreement that takes effect on or before
  the last day of a period a standing invoice or entry names: it was made
  under the version then in force, which is derived, and so must stay in
  force (§2.9).
- **N11** An agreement not active stays as it is: no version is added to it,
  and its status does not change (§2.9).

The conditions hold of every operation but `undo`, which takes back the last
one and makes no change of its own: a period closed by mistake is reopened by
undo, while nothing has happened since.

### 4.10 Partners and agreements

- **I-T1** Partner and agreement ids are well formed: one or more of a-z, 0-9
  and `-`. A partner is named on their first line, and their tax number, if
  any, is one line.
- **I-T2** What names a partner or an agreement names one there is: a bill's
  partner, an invoice's `to`, a plain entry's agreement, an agreement's
  parties, a schedule's party, the agreement one is subsumed by, a
  document's version.
- **I-T3** An agreement has a party or more, none twice, and a version or
  more; a schedule's party is one of its parties. A version's `from` is on
  or after the `from` of the version added before it, and its `until`, if
  it has one, on or after its `from`. An ended agreement's day is on or
  after the day before its first version's `from`: one ended before it
  began has no periods. A schedule's `every` is 1 or more, and its draft
  holds nothing I-C1 refuses in a text, and no field but the form's it
  fills: the invoice form's for a schedule of invoices, the entry form's
  for one that pays or receives.
- **I-T4** An invoice or a plain entry that names an agreement has a
  period, and it is one of the agreement's periods, of a version whose
  schedule invoices for an invoice, and pays or receives for an entry; no
  two standing invoices or entries name the same agreement and period.
- **I-T5** An agreement is not subsumed by itself, nor by one subsumed,
  through others, by it.

---

## 5. Operations

Each operation is `op(m, args) → m'`, defined when its preconditions hold.
`apply` builds `m'`, then checks the invariants on it (`check`) and the
conditions on the change, before anything is written; if one fails, the
operation is refused with its number and nothing is written. Most
preconditions are therefore not listed: they are the invariants. The few that
are not — the debt paid is a debt, the statement removed exists — refuse as
`OP`.

A refusal is said of the operation. `check` says what is wrong with a state,
which is what is wanted of books read from disk; of an operation, it would
speak of the books after it, of an entry not yet there. So `apply` passes the
violations through `explain`, which keeps each one's number and says it of
what the operation was given and of the books before it — `the postings sum
to 1.00 EUR, not nought`, not `entry 17 does not balance`; `only 100.00 EUR
is open on bill 19 (Hosting)`, not `19 is paid more than it owes` — and leaves
out those that only follow from another it keeps: a commodity removed while
in use is not also said to have too few places for every amount in it.
Violations the books held before the operation are kept as `check` says them.

| Operation | Adds or changes |
|---|---|
| `post(date, description, postings, for)` | a `Plain` entry; `for`, when given, the agreement and the period of its schedule it is made for |
| `file(bytes, date, issuer, for)` | a `Document`, by the bytes' hash; `for`, when given, a `Bill` entry or an agreement's version it is the document of |
| `unfile(document)` | removes a `Document` that names no standing bill and no version: one filed by mistake, or a reverted bill's |
| `bill(document, description, postings)` | a `Bill` entry, its partner, when given, among what it says besides; `document`, when given — filed already, naming no bill or a reversed one — now names it |
| `invoice(customer, lines, rate, …)` | an `Invoice` entry with its content and its PDF's hash; `to` and `period`, when given, in its content, and the customer and their tax number, when not given, copied from the partner `to` names; the accounts, terms, payment options and, in another commodity, the price, as configured and declared now. `rate`, when not given: nought to a customer whose tax number names another country than the company's does (a country's code, then a digit, or `ATU`, `CHE`); the configured rate otherwise, whatever commodity the invoice is in, which says nothing of where the customer is |
| `credit(invoice, note)` | a `CreditNote` entry, reversing the invoice, and its PDF |
| `pay(debt, date, amount, from, cost)` | a `Payment` entry, from a money account; `cost`, what was paid, when in another commodity |
| `write_off(debt, date, to)` | a `Payment` entry of what is open, from the account `to` |
| `revert(entry, date)` | a `Reversal` entry |
| `record(account, commodity, date, balance, postings)` | a `Statement` clearing those postings |
| `unrecord(account, commodity)` | removes the latest statement of the account in the commodity; an earlier one cannot go, since later ones count what it cleared (I-S3) |
| `config_set(key, value)`, `config_unset(key)` | `config` (§5.2) |
| `company(name?, address?, country?, tax_id?)` | the company's details given, trimmed; a payment option held in its old name is held in its new one |
| `option_add(id, option)` | a payment option, in its canonical form (an IBAN in groups of four, a BIC in capitals, lines without blank ones), and the account it names, declared if it is not, as `init` declares it; refused if the id is taken |
| `option_edit(id, new_id?, change)` | the option `id`, renamed `new_id`, with each field `change` gives, and its account declared as `option_add` does; another kind keeps only what every kind has, a label or account that was the old kind's default becoming the new one's; refused if `id` names none or `new_id` is taken |
| `option_remove(id)` | the option `id`; refused if it names none |
| `account_add(account)`, `account_remove(account)` | `accounts` |
| `commodity_add(commodity, places)`, `commodity_remove(commodity)` | `commodities` |
| `price_add(date, commodity, amount)`, `price_remove(date, commodity)` | `prices` |
| `partner_add(id, lines, tax_id, employee)` | a `Partner`; refused if the id is taken |
| `partner_edit(id, lines?, tax_id?, employee?)` | the partner's details given; no invoice changes |
| `partner_remove(id)` | the partner, while nothing names them (I-T2) |
| `agreement_add(id, parties, version)` | an active `Agreement` of one version; refused if the id is taken |
| `amend(agreement, version)` | a version, added after the agreement's last |
| `agreement_end(id, status)` | the agreement's status: terminated, or subsumed by another, on a day |
| `agreement_remove(id)` | an agreement nothing names: one added by mistake |
| `undo()` | the state before the last operation (§5.3) |

No operation changes an entry once added, nor an agreement's version. Every
one adds something; removes a document that names no bill, an account's
latest statement, or a partner or agreement nothing names; links a document
to its bill; or changes the configuration, the declarations, a partner's
details or an agreement's status.

### 5.1 Declarations

Accounts, commodities and prices are declared by commands, as payment options
are, and listed and removed the same way:

```
abo account   add <Account>                list   remove <Account>
abo account   move [--date=<date>] <from> <to>
abo commodity add <CODE> <places>          list   remove <CODE>
abo price     add <date> <CODE> <amount>   list   remove <date> <CODE>
```

In the browser, one page lists the accounts, as their paths nest them, each
with its balance; the commodities; and the prices, each with Add and Remove.
An account dragged onto another of its class is moved under it, once the
rearrangement is confirmed: a move each, in the order made. What each refuses is what the invariants refuse:

| Refused | Because |
|---|---|
| `account add Foo:Bar` | its first segment names no class (I-A0) |
| `account add Assets:Bank:Cash` while `Assets:Bank` holds postings | postings go to leaves alone (I-A2) |
| `account remove` of one an entry, a statement or the configuration names | I-A1 |
| `commodity add eur 2` | three or more capital letters (I-K0) |
| `commodity remove` of one in use | I-A1 |
| `price add` or `price remove` dated on or before `closed_through` | N2 |
| `account move` of a class, into another class, or under itself | a balance says what its class is |
| `account move` onto an account that holds a balance | the two would be one; onto one at nought, such as the name it had before, it goes |
| `account move` of an account a standing debt is owed on | a payment takes off the debt's own account |

There is no edit, and no rename in place. An account's name is in every
posting to it, and entries never change; a commodity's places are in every
amount written in it. So an account that holds postings keeps its name, and a
commodity in use keeps its places; one not yet used is removed and added
again. `account move from to` is the move by an entry: `from` and every
account under it are declared under `to` in their stead, one entry dated
`date` moves each balance there, and the configuration names the new
accounts; each old one stays where an entry or a statement names it, at
nought, and is taken out where nothing does. Earlier periods and statements
keep the old names. A VAT box's query is left as written: one naming a moved
account is set again. A price removed changes no invoice:
each holds the price it used.

### 5.2 Configuration

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
`report.vat.<code>.label` and `.query`. Each key has a shape, from
`abo.toml`'s schema, and its value is read as that shape and nothing else: a
key that names a table — `vat`, `payment.<id>`, `report.vat.<code>` — takes
its keys together, `{ … }`, for what cannot change one key at a time, such as
the VAT method and its accounts; `invoice.terms_days` takes a whole number;
every other key takes text, an address beginning with `{` as well:

```
abo config set vat '{ method = "flat", rate = "6.5", account = "Liabilities:VAT" }'
```

Every change is checked by I-C1 to I-C3 and the invariants they bear on, and
refused with the number of the one it would break; a key that names nothing,
or a value not of its key's shape, is refused. `abo company` and `abo
payment-options` stay, to ask what they need as questions; each answer is a
`config_set`.

### 5.3 Undo

Undo restores the state before the last operation, and is defined only while
that operation is the last: undo of anything older is not an operation. An
older mistake is taken back with `revert`, which adds the opposite and
removes nothing, as bookkeeping does it. In the model, undo is a function of
the states after and before the last operation, `undo(after, before)`, which
whoever holds the states keeps — the git history, in the books:

- It is `before`, but that the ids `after` gave stay given: an entry the
  operation added leaves a gap where it was, so its id is never given again.
- It is not defined where the operation issued an invoice or a credit note:
  one issued may be with the customer already, and is cancelled by a credit
  note (F5), not taken back.

This replaces today's undo, which reverses the text of any past commit. Over a
state, "the last operation" is well defined; "the text of an old commit,
reversed" is not, and is where the chains the verification runs found came
from: settled twice, paid more than owed, a reversal naming what is gone, a
statement over postings no longer cleared. Undo of an undo is redo of the
last operation, and is allowed while nothing has happened since.

### 5.4 The log

Each operation appends its line to `.abo/log.jsonl` as today: the command and
its stamp. The log is a record of what was done, not a source of the state:
the state is read from the files alone. Replaying it needs what the commands
read besides — the documents filed, the templates as they were — so the git
history, not the log, is what restores a state.

Two operations may be made as one change: the second applied to the state
the first leaves, each checked as if alone, neither made if either is
refused. The change is one commit and two lines of the log, each its own
operation's command; undo takes back both, as the change was one. The
browser's **File and book it** is the one such pair: a `file` and the
`bill` that names its document.

### 5.5 Partners and agreements

The commands and pages are not built yet; the operations are (§5).
Partners and agreements are kept by commands, as declarations are:

```
abo partner   [list]   add <id> [--name --address... --tax-id --employee]
              edit <id> [--name --address... --tax-id --employee=yes|no]
              remove <id>
abo agreement [list]   show <id>   remove <id>
abo agreement add <id> --partner=<id>... --from=<date> [--until=<date>]
                       [--invoice|--pay|--receive=<partner> [--every=<months>]
                        --in-advance|--in-arrears
                        [--draft=<field>=<value>... | --draft-from=<invoice or entry>]]
abo agreement amend <id> --from=<date> ...  [--no-schedule]
abo agreement terminate <id> [--on=<date>]  subsume <id> --by=<id> [--on=<date>]
abo file    --for=<id>/<n> ... <file>
abo bill    --partner=<id> ...
abo invoice --partner=<id> ...
abo invoice --agreement=<id> --period=<from>..<to> ...
abo post    --agreement=<id> --period=<from>..<to> ...
```

`partner add` and `edit`, given no flags, ask for the name, then the
address and the tax number, either of which may be left blank. `amend`
takes what `add` takes, and what it is not given it keeps from the version
in force on its `--from`: `--until=` given empty ends none, and
`--no-schedule` keeps no schedule. `--partner` is given once for each
party; `--invoice`, `--pay` or `--receive` names the party the schedule is
with; `--every` is 1 unless given. `--draft=<field>=<value>` sets a field
of the draft, and again for the same field adds a line to it; `<field>=`
alone clears it. `--draft-from` fills the draft from an issued invoice's
content, as **Issue again** fills the form, or from an entry's, as the
entry form is filled from one. `terminate` and `subsume` end it on the day
they are run, unless `--on` says another. `abo file --for` names a bill by
its id, or an agreement's version as `<id>/<n>`. An invoice or an entry
for a period of an agreement takes what it is not given from the draft of
the version in force on the period's first day; logged, what was left
blank is written out blank, so that run again it does not take the
draft's. An invoice issued with `--partner` or `--agreement` is logged with the
customer and tax number it copied, written out: run again after the
partner's address has changed, it issues the same invoice (G-O8).

In the browser, **Partners** lists each partner with what they owe and are
owed, their invoices, bills and agreements, the employees apart, each one
no active agreement pays marked; an agreement's page, its parties, status,
versions, documents and periods. **Invoices** lists the periods due to
invoice above the invoices, and the home page those due to pay or receive
above the entries, each opening its form filled in from its draft and
period, dated today. Each invoice has **Issue again**, the form filled in from it.

---

## 6. What changes for workflows

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

## 7. The canonical form

### 7.1 Files

```
abo.toml                     # the configuration, written by render
journal/main.journal         # includes accounts.journal, then each year, in order; then, when the last id given is a gap, `; next id: N`
journal/accounts.journal     # declarations, written by render
journal/2026.journal         # the year's entries and statements
invoices/2026/2026-0017.json # invoice 2026-0017's content and its PDF's hash, written by render
invoices/2026/2026-0017.pdf  # made at issue, kept as made
bills/2026/…                 # bills' documents, kept as they came; known by their hash
bills/index                  # every bill's document: its hash, path and bill, written by render
partners.toml                # partners, written by render
contracts/acme-hosting.toml  # an agreement: its parties, status and versions, written by render
contracts/acme-hosting/…     # an agreement's documents, kept as they came; known by their hash
contracts/index              # every agreement's document: its hash, path and version, written by render
templates/…                  # the company's, not the model's (§10)
```

`render` writes the text files: `abo.toml`, the journal, each invoice's and
credit note's `.json`, `partners.toml`, each agreement's `.toml`, and the two
indexes: `bills/index`, a line for each bill's document, `<hash> <path>
<entry id or ->`, and `contracts/index`, a line for each agreement's, `<hash> <path>
<agreement>/<version>`, each in the order of the hashes. An invoice's
`.json` holds its content and its PDF's hash; a credit note's, its note and its
PDF's hash. The two kinds of binary file are not rendered — a bill's document
is kept as it came, an invoice's PDF as it was made — and the state holds the
hash of each, so `parse` can tell one that is missing or changed.

Every file above is in the books' git repository, and committed with the
operation that writes it: an agreement's `.toml` and its documents under
`contracts/` as much as the journal.

### 7.2 An entry

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
| `Plain` | none, or, made for a period of an agreement, `agreement: <id>`, `period: <from>..<to>` and `partner: <id>` |
| `Bill` | `bill:`, then `number: <text>`, `due: <date>` and `partner: <id>` where given |
| `Invoice` | `invoice: <number>`, then `partner: <id>` where it has one; its content in its `.json` |
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
differs from the derived one is not canonical (§8).

### 7.3 A statement

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

### 7.4 Order and form

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

### 7.5 abo.toml

Written by `render` from `config`, keys in a fixed order, values
in their canonical forms (an IBAN in groups of four, a BIC in capitals). In
`abo.toml` and the `.json` files a number is written in its shortest form,
`8.1`, not `8.10`, so that one value is one text. Only
abo writes it: every change goes through `abo config`, `abo company` and `abo
payment-options`; a comment written in it by hand, like any other hand edit,
is a problem `parse` reports (§8), and holds every change up until mended.

### 7.6 partners.toml and contracts/

Written by `render`. `partners.toml` holds the partners in the order of
their ids, `tax_id` only when given, `employee` only when true; lines more
than one are written as TOML's multi-line string:

```toml
[partner.acme]
lines = """
Acme GmbH
Hauptstrasse 1
10115 Berlin
Germany"""
tax_id = "DE123456789"

[partner.jan]
lines = "Jan de Vries"
employee = true
```

`contracts/<id>.toml` holds one agreement: its parties as given, its status
— `active`, `terminated` with its day, or `subsumed` with the agreement and
`on` — and its versions in the order added, each date a TOML date, a
schedule's draft the form's fields by their names, in the order of the
names, those blank left out:

```toml
parties = ["acme"]
status = "active"

[[version]]
from = 2026-01-01
invoice = "acme"
every = 1
billed = "in-advance"

[version.draft]
items = "1|110|Hosting"
reference = "PO 4471"

[[version]]
from = 2027-01-01
invoice = "acme"
every = 3
billed = "in-advance"

[version.draft]
items = "1|320|Hosting, a quarter"
reference = "PO 4471"
```

```toml
parties = ["jan"]

[status]
terminated = 2026-12-31

[[version]]
from = 2026-03-01
pay = "jan"
every = 1
billed = "in-arrears"

[version.draft]
description = "Salary"
postings = """
Expenses:Salaries=3000.00 EUR
Liabilities:Wage tax=-600.00 EUR
Assets:Bank=-2400.00 EUR"""
```

A subsumed agreement's status is `[status]` with `subsumed = "<id>"` and
`on = <date>`. A version without a schedule has `from` and, if it ends, `until`, and
nothing else. An invoice's and a plain entry's `partner:` tag is derived,
written for hledger and a person reading the journal, so that `hledger reg
tag:partner=acme` lists a partner's bills, invoices and entries; a bill's is
what the bill says (§2.4).

---

## 8. Reading what abo did not write

`read` reads every file into a state and a list of problems. A problem is a
place — file and line — and what is wrong with it: not readable (`parse` fails
there), breaking an invariant (`check` names it), or not canonical. Files are
canonical exactly when `render(parse(files)) = files`, so that last list needs
no rules of its own: it is where the two differ.

- **Reading never stops on a problem.** Pages and reports show the books, and
  every page says that the books hold problems, with a link to the list.
- **On books that break a rule, an operation is made only if it mends.** It
  is refused for a violation the books after it hold and the books before it
  did not; and while the books before it hold a violation, it is made only
  if the books after it hold fewer. So declaring an account in use, once its
  declaration was taken out, goes ahead; an entry booked on top of one that
  does not balance does not. While text is not as abo writes it, nothing is
  changed until `abo tidy` has written it so, and said what it changed: a
  change would rewrite it unasked — a hand edit of an invoice's derived
  postings among it.
- **Changes not committed hold every operation up, tidying included.** abo
  commits every change it makes, so a change not committed is someone
  else's, and theirs to settle: abo asks them to commit it themselves or to
  take it out, naming the files and the git commands, and changes nothing
  until they have. It says what the change breaks against the last commit,
  for information, and offers no abo command as a way out of it: abo's
  commit would sign the change as abo's own.
- **`abo check`** lists the problems, each at its file and line — an entry at
  its date line, an account where it is first posted to, a statement at its
  own line — and says how each is mended: changes not committed, by settling
  them first; then an undeclared account by declaring it, what is not
  canonical by tidying, the rest by hand at the line named, and committed.
- **`abo tidy`** rewrites what is committed canonically, as one commit, and
  says what remains that tidying cannot mend.

This replaces both today's refusal to read an `abo.toml` it would not have
written, and its sweeping a hand edit into its own commit.

---

## 9. Books from before

Books written before the model — under rules 1 to 4 — are not migrated: no real
books were written then, only the demo, which is rebuilt on every start, and
the tests', which are rebuilt with the code. `parse` reads such books as far as
it can and reports the rest as not canonical (§8). Should real books from before
ever turn up, a converter is written then, for what they hold.

With the model, one set of invariants holds, and the 76 rule-version switches are gone.

---

## 10. What the model does not cover

- **Templates** (`templates/*.typ`) are the company's, edited by hand, and not
  part of the state. An invoice's content and its PDF's hash are.
- **The log** and the rest of `.abo/` are working state, as today.
- **Reports** are functions of the state, not part of it.
- **Users** (M5) would add an author to each operation, not new state.
- **Payroll** is booked, not computed: what is withheld and owed on a salary
  is said by whoever runs it, in the entry its schedule's draft starts.

---

## 11. Testing

Every state an operation produces passes `check`, by construction: `apply`
keeps no other. What the tests check is what construction does not give. They
are properties (`src/model/tests.rs`, with proptest), each over plans of up to
60 operations on fresh books.

Each property tests a guarantee of §3, whose tables name them; that every
property there exists, and every property is there, is itself a test,
`every_guarantee_is_tested`.

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

The commands are tested the same way, through the binary
(`src/cli_tests.rs`): the model tests' plans, run as `abo` commands in books
`abo init` opened, beside `model::apply` on the same operations. After every
step, abo and the model agree on whether it is taken; the files on disk are
what `render` writes for the model's state; and a refusal names the model's
rules and changes nothing. The other CLI properties are those §3 names under
G-F5 and G-O5 to G-O7. The web pages are tested so too (`src/web_tests.rs`):
the same plans posted as their forms to a running `abo serve`, under G-W1 to
G-W4.

What the properties found, each now a rule above:

- hledger takes a total cost's sign from its amount: a cost written with a
  sign was read as the opposite one. Costs are written unsigned (§7.2).
- hledger wants a decimal point in a commodity directive: a commodity of no
  places is declared `commodity 1000. JPY` (§7.4).
- Numbers past an exact decimal's range made the model panic: I-K3.
- Dates outside four-digit years were written as nothing could read: I-D1,
  and years are written with four digits, `0841`.
- A plain entry to the receivable before the first invoice made the balance
  and what is open disagree: N5 became I-P3.
- Reading an edited `bills/index` panicked on a hash not in ASCII: `read` is
  total on any text.
- A write-off could go to a VAT account, and so change the VAT reclaimed or
  owed: N3 covers payments and write-offs too. Found by the property for
  G-P3, the first written from the list of guarantees.
- I-I2 compared an invoice's date with the one numbered just before it, so an
  invoice of the year before, between two of this year's, let them be dated
  out of order: it now compares within the year.
- An invoice's total could pass the bound its lines kept, and so be owed in
  an amount no payment could hold: I-K3 bounds totals too.
- Two documents of one issuer and day whose hashes begin alike had one path:
  I-B1 refuses the second.
- hledger reads every Unicode space in an account's name — a no-break space
  among them — as U+0020: two names abo told apart were one to hledger. A
  segment holds no other space (§2.2). Which characters hledger reads as
  written, where, is now a table asked of hledger for every character
  (§2.3), not a list of exceptions found one by one; asked, it showed that
  abo had refused `;` in an account's name, which hledger reads as written.
- abo began an invoice's description with the customer's name, so a customer
  named `(Acme) GmbH` was read by hledger as a code, and refused: the number
  comes first (§7.2). A customer whose first line was blank was refused by the
  description's rule, not the invoice's: I-I1 asks for a name.
- An amount at the bound in a commodity of 17 or 18 places is 29 or 30 digits,
  past what an exact decimal holds; and a sum of amounts of 16 places would
  lose its last digits, silently: I-K0 allows 8, which leaves 8 digits for
  sums.
- `render` rounded an amount to its commodity's places, so `abo tidy` changed
  an amount with more places than declared, or in a commodity not declared:
  it pads, and never rounds.
- `config set` read every value beginning with `{` as a table, so an address
  beginning with one could not be set: a value is read as its key's shape,
  which the schema gives (§5.2).
- A problem at a file's end — its last newline gone — named a line past it.

The 172 regression tests stay; most become tests of one invariant each, once
the commands are built on the model.
