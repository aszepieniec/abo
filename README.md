# abo

abo keeps the books of one small company in a git repository: an hledger
journal, the company's configuration (`abo.toml`), and the documents filed
with each entry. Every change to the books is one `abo` command, committed by
abo, and the log records it as that command; the commands of the log, run
again in an empty directory, make the same books. hledger computes every
figure in a report.

![abo opening new books, booking two entries, and showing the profit and loss and the log](docs/demo.gif)

<!-- docs/demo.gif and docs/demo.cast are made by scripts/demo.py: run it
     again after a change to what these commands print. -->

abo is also the library under
[abo-web](https://github.com/aszepieniec/abo-web), the web interface to the
same books.

## Requirements

- Rust, to build.
- `git` and `hledger` on the `PATH`.
- `typst` on the `PATH`, for invoice and report PDFs (optional).

## Build

```sh
git clone https://github.com/aszepieniec/abo
cd abo
cargo build --release
```

## Use

```sh
mkdir acme && cd acme
abo init Acme Ltd --currency EUR --vat none
```

opens new books in the empty directory. Without flags, `abo init` asks its
questions instead. Then:

```sh
abo account add Equity:Capital
abo post --desc "Capital paid in" Assets:Bank=1000 Equity:Capital=-1000
abo pl --period 2026
abo log
```

`abo help` lists every command. `--json` gives any command's output as JSON,
and `--ni` makes a command ask nothing, for scripts.

MODEL.md defines what the books are, DESIGN.md how abo is built and why, and
WORKFLOWS.md what it is for.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
