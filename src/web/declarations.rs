//! Accounts, commodities and prices declared: "Accounts" on the page.

use super::*;

/// The accounts, commodities and prices, each with Add and Remove (§5.1).
pub(super) async fn declarations(Books(s): Books) -> Response {
    or_error(&s, "Accounts", || Ok(declarations_page(&s, None)))
}

pub(super) fn declarations_page(s: &Store, error: Option<&anyhow::Error>) -> Markup {
    let b = &s.books;
    let remove = |what: &str, value: &str| if !may_change() { html! {} } else { html! {
        form method="post" action="/declarations" {
            (token())
            input type="hidden" name="action" value={ "remove-" (what) };
            input type="hidden" name="value" value=(value);
            button.small.btn-danger type="submit" { (icon("trash")) " Remove" }
        }
    } };
    let t = Tree::new(b);
    html! {
        h1 { "Accounts" }
        @if let Some(e) = error { (refused(e)) }
        div.box {
            div.box-header { (icon("book")) "Accounts" @if may_change() { span.spacer.muted.small { "drag " (icon("grip")) " onto another account to move it there" } } }
            ul.tree { @for c in model::CLASSES { (t.node(c)) } }
            @if may_change() { form #moves method="post" action="/declarations" hidden {
                (token()) input type="hidden" name="action" value="move-accounts";
                input type="hidden" name="value";
                div.box-body {
                    p.small { "Each account moved is declared under its new parent, and its balance moved there by one entry dated today; the old one stays where entries post to it." }
                    ul.small #move-list {}
                    div.actions { button.btn-primary type="submit" { "Confirm" } a.btn href="/declarations" { "Cancel" } }
                }
            }
            script { (PreEscaped(DRAG)) } }
        }
        div.box {
            div.box-header { (icon("coins")) "Currencies and Commodities" }
            table.list { tbody { @for (c, p) in &b.commodities { tr { td { code { (c) } } td { (p) " places" } td.amt { (remove("commodity", c)) } } } } }
            @if may_change() { div.box-body { details.add {
                summary.btn.small { (icon("plus")) " New currency or commodity" }
                form.one-row method="post" action="/declarations" {
                    (token()) input type="hidden" name="action" value="add-commodity";
                    label.field { "Code" span.hint { "Capital letters, such as CHF or BTC." } input type="text" name="value" required; }
                    label.field { "Places" span.hint { "Digits after the point, 0 to 8." } input type="text" name="places" required value="2"; }
                    button.btn-primary type="submit" { (icon("plus")) " Declare" }
                }
            } } }
        }
        div.box {
            div.box-header { (icon("chart")) "Prices" span.spacer.muted.small { "what one unit of a currency was worth, for invoices in it" } }
            table.list { tbody { @for ((c, d), p) in &b.prices { tr { td.date { (d) } td { "1 " (c) " = " (p.quantity) " " (p.commodity) } td.amt { (remove("price", &format!("{d} {c}"))) } } } } }
            @if may_change() { div.box-body { details.add {
                summary.btn.small { (icon("plus")) " New price" }
                form method="post" action="/declarations" {
                    (token()) input type="hidden" name="action" value="add-price";
                    @let home = &b.config.company.currency;
                    div.one-row {
                        label.field { "One unit of" select name="value" required {
                            @for c in b.commodities.keys() { option value=(c) selected[Some(c) == b.commodities.keys().find(|k| *k != home)] { (c) } }
                        } }
                        label.field { "Was worth" input type="text" name="price" inputmode="decimal" placeholder="60000.00" required; }
                        label.field { "In" select name="price_in" required {
                            @for c in b.commodities.keys() { option value=(c) selected[c == home] { (c) } }
                        } }
                    }
                    div.one-row {
                        label.field { "On" input type="date" name="date" required value=[store::today().ok()] max=[store::today().ok()]; }
                        button.btn-primary type="submit" { (icon("plus")) " Add" }
                    }
                }
            } } }
        }
    }
}

/// The accounts as their paths nest them, each with its balance in the
/// company's currency: another commodity at its latest price in it, and so
/// about; one with no such price left out, and said to be.
struct Tree<'a> {
    b: &'a model::Books,
    /// Every declared account, and every path above one.
    paths: std::collections::BTreeSet<String>,
    /// By path: in the company's currency; whether a price went into it;
    /// what no price converts.
    sums: std::collections::BTreeMap<String, (Decimal, bool, std::collections::BTreeMap<String, Decimal>)>,
    /// What an entry posts to, a statement is of, or the configuration
    /// names for postings: a leaf, so nothing is declared under it (I-A2),
    /// and kept, as what names it would name nothing (I-A1). Why, by account.
    leaves: std::collections::BTreeMap<String, &'static str>,
    /// What a move left behind: where to, and when. Read from the entries
    /// `account move` books, which say so; one moved back holds its balance again.
    moved: std::collections::BTreeMap<String, (String, model::Date)>,
    /// What a standing debt is owed on, and why it stays: a payment takes
    /// off the debt's own account, so what holds one is not moved.
    owed: std::collections::BTreeMap<String, String>,
    /// What holds a balance itself: nothing moves onto it.
    held: std::collections::BTreeSet<String>,
}

impl<'a> Tree<'a> {
    fn new(b: &'a model::Books) -> Tree<'a> {
        let up = |a: &str| a.match_indices(':').map(|(i, _)| a[..i].to_string()).chain([a.to_string()]).collect::<Vec<_>>();
        let paths = b.accounts.iter().flat_map(|a| up(a)).collect();
        let home = &b.config.company.currency;
        let mut sums = std::collections::BTreeMap::new();
        for ((a, c), q) in b.balances() {
            // Prices are dated today or before (N1): the last is the latest.
            let price = b.prices.range((c.clone(), model::Date::MIN)..=(c.clone(), model::Date::MAX)).rev().find(|(_, p)| &p.commodity == home).map(|(_, p)| p.quantity);
            for p in up(&a) {
                let s: &mut (Decimal, bool, std::collections::BTreeMap<String, Decimal>) = sums.entry(p).or_default();
                match price {
                    _ if &c == home => s.0 += q,
                    Some(r) => {
                        s.0 += q * r;
                        s.1 = true;
                    }
                    None => *s.2.entry(c.clone()).or_default() += q,
                }
            }
        }
        // The first reason said: what is written last.
        let leaves = (b.config_accounts().into_iter().map(|a| (a, "the company's settings name it")))
            .chain(b.statements.keys().map(|k| (k.0.clone(), "a statement is recorded for it")))
            .chain(b.all_postings().into_iter().map(|(_, _, p)| (p.account, "entries post to it")))
            .collect();
        let under = |a: &str, b: &str| a == b || a.strip_prefix(b).is_some_and(|r| r.starts_with(':'));
        let mut moved = std::collections::BTreeMap::new();
        for e in &b.entries {
            let (model::Kind::Plain(ps, _), Some(rest)) = (&e.kind, e.description.strip_prefix("Move ")) else { continue };
            // A name may hold " to ": the split whose first part the postings are under.
            let Some((from, to)) = rest.match_indices(" to ").map(|(i, _)| (&rest[..i], &rest[i + 4..])).find(|(f, _)| ps.iter().any(|p| under(&p.account, f))) else { continue };
            for p in ps.iter().filter(|p| under(&p.account, from)) {
                moved.insert(p.account.clone(), (format!("{to}{}", &p.account[from.len()..]), e.date));
            }
        }
        let standing = b.standing();
        let mut owed = std::collections::BTreeMap::new();
        for d in 1..=b.entries.len() {
            if let Some(x) = b.debt(d).filter(|_| b.open(d, &standing) > Decimal::ZERO) {
                owed.entry(x.account.clone()).or_insert_with(|| format!("{} is owed on {}; pay or write it off first.", model::name(b, d), x.account));
            }
        }
        let held = b.balances().into_keys().map(|(a, _)| a).collect();
        Tree { b, paths, sums, leaves, moved, owed, held }
    }

    fn balance(&self, path: &str) -> Markup {
        let home = &self.b.config.company.currency;
        let Some((q, about, rest)) = self.sums.get(path) else { return html! {} };
        let rest: Vec<(&String, &Decimal)> = rest.iter().filter(|(_, q)| !q.is_zero()).collect();
        let left: Vec<&str> = rest.iter().map(|(c, _)| c.as_str()).collect();
        html! {
            @if !q.is_zero() || *about || rest.is_empty() {
                @if *about { span title="at the latest prices" { "\u{2248}\u{a0}" } }
                (money_of(self.b, model::round(*q, self.b.places(home)), home))
                @if !rest.is_empty() { " " span.muted.small title={ "not counted: no price in " (home) } { "+\u{a0}" (left.join(", ")) } }
            } @else {
                // Nothing in the company's currency: its own amounts, which no price converts.
                @for (i, (c, q)) in rest.iter().enumerate() { @if i > 0 { ", " } (money_of(self.b, **q, c)) }
            }
        }
    }

    fn node(&self, path: &str) -> Markup {
        let under = format!("{path}:");
        let children: Vec<&String> = self.paths.range(under.clone()..).take_while(|p| p.starts_with(&under)).filter(|p| !p[under.len()..].contains(':')).collect();
        let declared = self.b.accounts.contains(path);
        let class = model::class(path);
        let last = path.rsplit(':').next().unwrap_or(path);
        html! {
            @let gone = self.moved.get(path).filter(|_| declared && !self.sums.contains_key(path));
            li.gone[gone.is_some()] data-name=(path) data-class=(class) data-leaf[self.leaves.contains_key(path)] data-held[self.held.contains(path)] {
                details {
                    summary title=[gone.map(|(to, d)| format!("Moved to {to} on {d}. It stays, at nought, as the entries before then post to it, so it cannot be removed."))] {
                        span.chev {}
                        @if gone.is_some() { span.grip-gap {} }
                        @let stuck = self.owed.iter().find(|(a, _)| *a == path || a.starts_with(&under)).map(|(_, why)| why);
                        @if path != class && gone.is_none() && may_change() {
                            @if let Some(why) = stuck { span.grip.stuck title={ "It cannot be moved: " (why) } { (icon("grip")) } }
                            @else { span.grip draggable="true" title="Drag onto another account to move it there" { (icon("grip")) } }
                        }
                        span.name title=(path) { @if declared { a href=(account_href(path)) { (last) } } @else { (last) } }
                        span.bal { (self.balance(path)) }
                        @let kept = self.leaves.get(path);
                        // On the wrapper: a disabled button shows no title in every browser.
                        span.act title=[kept.map(|why| format!("{path} cannot be removed: {why}"))] { @if declared && may_change() {
                            form method="post" action="/declarations" {
                                (token())
                                input type="hidden" name="action" value="remove-account";
                                input type="hidden" name="value" value=(path);
                                button.small.btn-danger.icon-only type="submit" disabled[kept.is_some()] title=[kept.is_none().then(|| format!("Remove {path}"))] aria-label=(kept.map_or(format!("Remove {path}"), |why| format!("{path} cannot be removed: {why}"))) { (icon("trash")) }
                            }
                        } }
                    }
                    ul {
                        @for c in &children { (self.node(c)) }
                        // Any account takes another under it, but what postings go to (I-A2).
                        // Such a one with none under it shows no arrow: its list is empty.
                        @if !may_change() {
                        } @else if let Some(why) = self.leaves.get(path).filter(|_| !children.is_empty()) {
                            li.add { span.kept title={ "Nothing can be declared under " (path) ": " (why) ", and postings go to accounts with none under them." } {
                                button.btn.small type="button" disabled { (icon("plus")) " New account" }
                            } }
                        } @else if self.leaves.contains_key(path) {
                        } @else {
                            li.add { details {
                                summary.btn.small title={ "Declare an account under " (path) } { (icon("plus")) " New account" }
                                form.one-row method="post" action="/declarations" {
                                    (token()) input type="hidden" name="action" value="add-account";
                                    input type="hidden" name="parent" value=(path);
                                    label.field { "Name" span.hint { "Under " (path) ", as " code { "Rent" } " or " code { "Office:Rent" } "." } input type="text" name="value" required; }
                                    button.btn-primary type="submit" { (icon("plus")) " Declare" }
                                }
                            } }
                        }
                    }
                }
            }
        }
    }
}

/// Dragging an account by its grip onto another of its class puts it
/// there, and adds the move to the form that confirms them. Each move is
/// said by the names it had then, so the moves are made in their order.
/// The tree starts collapsed; which branches are open is kept only across
/// what this page reloads itself for: a form sent from it, or Cancel.
const DRAG: &str = r#"
(() => {
  const form = document.getElementById("moves"), list = document.getElementById("move-list"), moves = [];
  const key = "abo-open:" + location.pathname;
  const branches = () => document.querySelectorAll(".tree li[data-name] > details");
  let open = [];
  try { open = JSON.parse(sessionStorage.getItem(key) || "[]"); sessionStorage.removeItem(key); } catch (e) {}
  branches().forEach((d) => { d.open = open.includes(d.parentElement.dataset.name); });
  const keep = () => { try { sessionStorage.setItem(key, JSON.stringify([...branches()].filter((d) => d.open).map((d) => d.parentElement.dataset.name))); } catch (e) {} };
  document.querySelectorAll("main form").forEach((f) => f.addEventListener("submit", keep));
  document.querySelectorAll('main a[href$="/declarations"]').forEach((a) => a.addEventListener("click", keep));
  let dragged = null;
  // What the server would refuse: a new name that holds a balance, or one
  // that entries post to with accounts to go under it.
  const clash = (li) => {
    const from = dragged.dataset.name, to = li.dataset.name + ":" + from.split(":").pop();
    return [dragged, ...dragged.querySelectorAll("li[data-name]")].some((x) => {
      const there = document.querySelector(`.tree li[data-name="${CSS.escape(to + x.dataset.name.slice(from.length))}"]`);
      return there && ("held" in there.dataset || ("leaf" in there.dataset && x.querySelector("li[data-name]")));
    });
  };
  const ok = (li) => dragged && li && !("leaf" in li.dataset) && li.dataset.class === dragged.dataset.class && !dragged.contains(li) && dragged.parentElement.closest("li") !== li && !clash(li);
  document.querySelectorAll(".tree .grip").forEach((g) => {
    g.addEventListener("click", (e) => e.preventDefault());
    g.addEventListener("dragstart", (e) => {
      dragged = g.closest("li");
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", dragged.dataset.name);
      e.dataTransfer.setDragImage(g.closest("summary"), 0, 0);
    });
    g.addEventListener("dragend", () => { dragged = null; document.querySelectorAll(".tree .over").forEach((x) => x.classList.remove("over")); });
  });
  document.querySelectorAll(".tree li[data-name] > details > summary").forEach((s) => {
    const li = s.closest("li");
    s.addEventListener("dragover", (e) => { if (ok(li)) { e.preventDefault(); s.classList.add("over"); } });
    s.addEventListener("dragleave", () => s.classList.remove("over"));
    s.addEventListener("drop", (e) => {
      e.preventDefault();
      s.classList.remove("over");
      if (!ok(li)) return;
      const from = dragged.dataset.name, to = li.dataset.name + ":" + from.split(":").pop();
      dragged.querySelectorAll("li[data-name]").forEach((x) => { x.dataset.name = to + x.dataset.name.slice(from.length); });
      dragged.dataset.name = to;
      dragged.querySelector(".name").title = to;
      const ul = li.querySelector(":scope > details > ul");
      ul.insertBefore(dragged, [...ul.querySelectorAll(":scope > li[data-name]")].find((x) => x !== dragged && x.dataset.name > to) || ul.querySelector(":scope > li.add"));
      li.querySelector(":scope > details").open = true;
      // A move of what an earlier one put there becomes part of that one,
      // when no move between them names anything under it; one that puts
      // it back undoes it.
      const within = (a, b) => a === b || a.startsWith(b + ":");
      const i = moves.findIndex((m, j) => m[1] === from && moves.slice(j + 1).every((n) => !within(n[0], from) && !within(n[1], from)));
      if (i < 0) moves.push([from, to]);
      else if (moves[i][0] === to) moves.splice(i, 1);
      else moves[i][1] = to;
      dragged.classList.toggle("moved", moves.some((m) => m[1] === to));
      form.elements.value.value = moves.flat().join("\n");
      list.replaceChildren(...moves.map(([f, t]) => { const item = document.createElement("li"); item.append(f + " → " + t); return item; }));
      form.hidden = moves.length === 0;
    });
  });
})();
"#;

pub(super) async fn declare(Books(mut s): Books, Form(f): Form<Fields>) -> Response {
    let get = |k: &str| f.get(k).map_or("", |v| v.trim()).to_string();
    let root = s.root.clone();
    let r = once(Some(&root), &get("token"), || {
        let v = match get("parent").as_str() {
            "" => get("value"),
            p => format!("{p}:{}", get("value")),
        };
        if get("action") == "move-accounts" {
            // In their order, each a change of its own, as each a command.
            let names: Vec<&str> = v.lines().collect();
            if names.is_empty() || !names.len().is_multiple_of(2) {
                bail!("nothing to move");
            }
            let date = store::today()?;
            let mut to = String::new();
            for m in names.chunks(2) {
                to = change(&mut s, &Op::AccountMove { from: m[0].into(), to: m[1].into(), date }, &[], "/declarations")?;
            }
            return Ok(to);
        }
        let op = match get("action").as_str() {
            "add-account" => Op::AccountAdd(v.clone()),
            "remove-account" => Op::AccountRemove(v.clone()),
            "add-commodity" => {
                let places: u32 = get("places").trim().parse().map_err(|_| anyhow::anyhow!("{} is not a number of places, such as 2", get("places")))?;
                Op::CommodityAdd(v.clone(), places)
            }
            "remove-commodity" => Op::CommodityRemove(v.clone()),
            "add-price" => {
                let date = input::day(Some(&get("date")))?;
                let price = input::amount(&s.books, &format!("{} {}", get("price"), get("price_in")))?;
                Op::PriceAdd { of: v.clone(), date, price }
            }
            "remove-price" => {
                let (d, c) = v.split_once(' ').context("not a date and a commodity")?;
                let date = input::day(Some(d))?;
                Op::PriceRemove { of: c.into(), date }
            }
            _ => bail!("nothing to do"),
        };
        change(&mut s, &op, &[], "/declarations")
    });
    match r {
        Ok(to) => Redirect::to(&to).into_response(),
        Err(e) => again(&s, "Accounts", || Ok(declarations_page(&s, Some(&e)))),
    }
}
