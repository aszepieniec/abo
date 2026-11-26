#!/usr/bin/env python3
"""Random sequences of `abo corporate` commands, names chosen to collide
(NFC and NFD, capitals, twins, .keep in any capitals, links, files not
committed, hand commits), each checked against a model of the tree kept
here, then the log replayed into new books and compared (G-C1, G-C2, G-O8).

Usage: corporate.py ABO WORKDIR FIRST_SEED END_SEED. Exits 1 on any issue,
printing each. tests/fuzz.rs runs it; ABO_FUZZ=n runs n times as many."""
import os, sys, subprocess, random, unicodedata, hashlib, json, shutil, stat, shlex, traceback

ABO = os.path.abspath(sys.argv[1])
ENV = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@x.invalid",
           GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@x.invalid", LC_ALL="C.UTF-8")
EMPTY = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"

def sh(args, cwd, inp=None, shell=False):
    p = subprocess.run(args, cwd=cwd, env=ENV, capture_output=True, input=inp, shell=shell, timeout=120)
    return p.returncode, p.stdout.decode("utf-8", "surrogateescape"), p.stderr.decode("utf-8", "surrogateescape")

def git(b, *a):
    return sh(["git", *a], b)

nfc = lambda s: unicodedata.normalize("NFC", s)
nfd = lambda s: unicodedata.normalize("NFD", s)
fold = lambda s: nfc(s).casefold()

def valid(n):
    if not n or len(n.encode()) > 255: return False
    if any(c in n for c in "/\\\n"): return False
    if n.startswith("."): return False
    if n[-1].isspace() or n.endswith("."): return False
    if any(unicodedata.category(c) == "Cc" for c in n): return False
    return True

def iskeep(n): return fold(n) == ".keep"

# ---------- snapshots ----------
def tree(b, rev="HEAD"):
    rc, out, _ = git(b, "ls-tree", "-r", "-z", rev)
    t = {}
    for rec in out.split("\0"):
        if not rec: continue
        meta, path = rec.split("\t", 1)
        mode, typ, sha = meta.split()
        t[path] = (mode, sha)
    return t

def ctree(t): return {p: v for p, v in t.items() if p.startswith("corporate/")}

def walk(b):
    res = {}
    for root, dirs, files in os.walk(b):
        rel = os.path.relpath(root, b)
        if rel == ".":
            dirs[:] = [d for d in dirs if d not in (".git", ".abo")]
        for n in dirs + files:
            p = os.path.join(root, n); r = os.path.normpath(os.path.join(rel, n))
            st = os.lstat(p)
            if stat.S_ISLNK(st.st_mode): res[r] = ("l", os.readlink(p))
            elif stat.S_ISDIR(st.st_mode): res[r] = ("d", stat.S_IMODE(st.st_mode))
            else:
                with open(p, "rb") as f: h = hashlib.sha1(f.read()).hexdigest()
                res[r] = ("f", stat.S_IMODE(st.st_mode), h)
        # don't descend into symlinked dirs (os.walk doesn't follow by default)
    return res

def snap(b):
    return dict(head=git(b, "rev-parse", "HEAD")[1].strip(),
                index=git(b, "ls-files", "-s", "-z")[1], files=walk(b))

def untracked(b, files, t):
    return {p: v for p, v in files.items() if v[0] != "d" and p not in t}

def blob(data):
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()

# ---------- model helpers ----------
def dirs_of(t):
    ds = set()
    for p in t:
        parts = p.split("/")
        for i in range(1, len(parts)):
            ds.add("/".join(parts[:i]))
    return ds

def children(t, d):
    pre = d + "/"; out = {}
    for p, v in t.items():
        if p.startswith(pre):
            rest = p[len(pre):]
            n = rest.split("/")[0]
            out[n] = "d" if "/" in rest else ("l" if v[0] == "120000" else "f")
    return out

class Either(Exception): pass
class Refuse(Exception): pass

def lookup(t, ds, comps):
    """exact (NFC) lookup of existing path; returns stored path or None"""
    cur = "corporate"
    for c in comps:
        ch = children(t, cur)
        m = [n for n in ch if nfc(n) == nfc(c)]
        if not m: return None
        if len(m) > 1: raise Either("nfc twins")
        cur = cur + "/" + m[0]
    return cur

def kind(t, p):
    if p in t: return "l" if t[p][0] == "120000" else "f"
    if p in dirs_of(t) or p == "corporate": return "d"
    return None

def has_twin(t, p):
    par, n = p.rsplit("/", 1)
    return any(fold(c) == fold(n) and c != n for c in children(t, par))

def disk_kind(b, p):
    full = os.path.join(b, p)
    try: st = os.lstat(full)
    except FileNotFoundError: return None
    except NotADirectoryError: return None
    if stat.S_ISLNK(st.st_mode): return "l"
    if stat.S_ISDIR(st.st_mode): return "d"
    return "f"

def parse_arg(a):
    """split a corporate path argument into components (no validity check)"""
    if a == "corporate" or a == "corporate/": return []
    if not a.startswith("corporate/"): raise Refuse("outside")
    comps = a[len("corporate/"):].rstrip("/").split("/")
    if any(c == "" for c in comps): raise Either("empty comp")
    return comps

def ensure_dir(t, b, comps, created, new_ok=True):
    """resolve/create a directory path given by user; return stored path."""
    cur = "corporate"
    for c in comps:
        if not valid(c): raise Refuse("bad name %r" % c)
        ch = children(t, cur)
        m = [n for n in ch if nfc(n) == nfc(c)]
        if m:
            if len(m) > 1: raise Either("nfc twins")
            n = m[0]
            nxt = cur + "/" + n
            if ch[n] != "d": raise Refuse("file on way")
            if has_twin(t, nxt): raise Refuse("twin dir")
            if not all(valid(x) for x in nxt.split("/")[1:]): raise Either("bad dir")
            cur = nxt
            continue
        if any(fold(n) == fold(c) for n in ch): raise Refuse("folded taken")
        nxt = cur + "/" + nfc(c)
        dk = disk_kind(b, nxt)
        if dk in ("f", "l"): raise Refuse("untracked on way")
        if dk == "d": raise Either("untracked dir on way")
        created.append(nxt)
        cur = nxt
    return cur

def keep_rules(t_new, t_old, b, created, emptied_ok=()):
    """returns (required set of .keep paths to add, set of dirs whose .keep is either)"""
    req, either, forbid = set(), set(), set()
    ds = dirs_of(t_new) | set(created)
    ds.discard("corporate")
    for d in ds:
        ch = children(t_new, d)
        if any(iskeep(n) for n in ch): continue
        comps = d.split("/")[1:]
        if not all(valid(c) for c in comps): forbid.add(d); continue
        if any(nfc(c) != c for c in comps): either.add(d); continue
        if has_twin(t_new, d): forbid.add(d); continue
        anc = ["/".join(d.split("/")[:i]) for i in range(2, len(d.split("/")))]
        if any(has_twin(t_new, a) for a in anc): either.add(d); continue
        # any untracked .keep-ish on disk at d
        dk = disk_kind(b, d + "/.keep")
        try:
            names = os.listdir(os.path.join(b, d))
        except Exception:
            names = []
        if dk is not None or any(iskeep(n) for n in names): forbid.add(d); continue
        if d in created and disk_kind(b, d) == "d": either.add(d); continue
        req.add(d)
    return req, either, forbid

def apply_keeps(t_new, req):
    t = dict(t_new)
    for d in req: t[d + "/.keep"] = ("100644", EMPTY)
    return t

# ---------- the model of each command ----------
def model(op, t, b, ctx):
    """returns (outcome, expected_tree, either_dirs, flexible_modes) where outcome in accept/refuse/either"""
    try:
        kind_ = op[0]
        created = []
        if kind_ == "mkdir":
            comps = parse_arg(op[1])
            if not comps: raise Refuse("corporate itself")
            ex = lookup(t, None, comps)
            if ex is not None or (ex is None and _folded_exists(t, comps)): raise Refuse("taken")
            parent = ensure_dir(t, b, comps[:-1], created)
            c = comps[-1]
            if not valid(c): raise Refuse("bad")
            p = parent + "/" + nfc(c)
            dk = disk_kind(b, p)
            if dk in ("f", "l"): raise Refuse("untracked at")
            if dk == "d": raise Either("untracked dir at")
            created.append(p)
            new = dict(t)
        elif kind_ == "add":
            srcs, dest = op[1], op[2]
            comps = parse_arg(dest)
            names = []
            for s in srcs:
                sp = os.path.join(ctx["src"], s)
                if not os.path.exists(sp) or os.path.isdir(sp): raise Refuse("bad src")
                n = nfc(os.path.basename(s))
                if not valid(n): raise Refuse("bad src name")
                names.append((n, open(sp, "rb").read()))
            if len({fold(n) for n, _ in names}) < len(names): raise Refuse("two one")
            d = ensure_dir(t, b, comps, created)
            ch = children(t, d)
            new = dict(t)
            for n, data in names:
                if any(fold(c) == fold(n) for c in ch): raise Refuse("taken")
                p = d + "/" + n
                dk = disk_kind(b, p)
                if dk is not None: raise Refuse("untracked at dest")
                if any(fold(x) == fold(n) for x in _disk_names(b, d)): raise Either("untracked folded")
                new[p] = ("ADD", blob(data))
        elif kind_ == "rm":
            paths = []
            for a in op[1]:
                comps = parse_arg(a)
                if not comps: raise Refuse("corporate itself")
                p = lookup(t, None, comps)
                if p is None: raise Refuse("not there")
                paths.append(p)
            if len(set(paths)) < len(paths): raise Refuse("twice")
            for p in paths:
                if any(p.startswith(q + "/") for q in paths): raise Either("nested rm")
            for p in paths:
                if iskeep(p.rsplit("/", 1)[1]):
                    raise Refuse("keep alone")
            new = {k: v for k, v in t.items() if not any(k == p or k.startswith(p + "/") for p in paths)}
            emptied = [d for d in dirs_of(t) - dirs_of(new) if d not in paths and not any(d.startswith(p + "/") for p in paths)]
            if emptied: raise Either("emptied dir without .keep")
        elif kind_ == "mv":
            srcs_a, to = op[1], op[2]
            srcs = []
            for a in srcs_a:
                comps = parse_arg(a)
                if not comps: raise Refuse("corporate itself")
                p = lookup(t, None, comps)
                if p is None: raise Refuse("not there")
                srcs.append(p)
            if len(set(srcs)) < len(srcs): raise Refuse("twice")
            for p in srcs:
                if any(p.startswith(q + "/") for q in srcs): raise Refuse("nested")
            for p in srcs:
                n = p.rsplit("/", 1)[1]
                if iskeep(n): raise Refuse("keep moved")
                if kind(t, p) == "l": raise Refuse("symlink moved")
                if kind(t, p) == "d":
                    inner = [k for k in t if k.startswith(p + "/")]
                    if any(t[k][0] == "120000" for k in inner): raise Either("symlink inside")
                    if any(not valid(c) for k in inner for c in k[len(p) + 1:].split("/") if not iskeep(c)): raise Refuse("bad name inside")
                    if any(nfc(c) != c for k in inner for c in k.split("/")): raise Either("nfd inside")
                if has_twin(t, p): raise Either("src has twin")
            tcomps = parse_arg(to)
            into = len(srcs) > 1 or to.endswith("/")
            if not into and tcomps:
                ex = lookup(t, None, tcomps)
                if ex is not None and kind(t, ex) == "d": into = True
            moves = []
            if into:
                d = ensure_dir(t, b, tcomps, created)
                for p in srcs:
                    if d == p or d.startswith(p + "/"): raise Refuse("into itself")
                ch = children(t, d)
                for p in srcs:
                    n = p.rsplit("/", 1)[1]
                    if not valid(n): raise Refuse("bad name carried")
                    np_ = d + "/" + nfc(n)
                    if np_ == p: raise Either("onto itself")
                    if any(fold(c) == fold(n) for c in ch): raise Refuse("taken")
                    moves.append((p, np_))
                if len({fold(m[1]) for m in moves}) < len(moves): raise Refuse("two one")
            else:
                if not tcomps: raise Refuse("to corporate")
                p = srcs[0]
                dparent = ensure_dir(t, b, tcomps[:-1], created)
                c = tcomps[-1]
                if not valid(c): raise Refuse("bad")
                np_ = dparent + "/" + nfc(c)
                if np_ == p or np_.startswith(p + "/"): raise Refuse("into itself")
                ch = children(t, dparent)
                same = [x for x in ch if fold(x) == fold(c)]
                if same:
                    if dparent + "/" + same[0] == p and same[0] != nfc(c): pass  # renamed in other capitals
                    else: raise Refuse("taken")
                moves.append((p, np_))
            new = dict(t)
            for p, np_ in moves:
                dk = disk_kind(b, np_)
                if dk in ("f", "l"): raise Refuse("untracked at dest")
                if dk == "d" and np_.lower() != p.lower(): raise Either("untracked dir at dest")
                for k in list(t):
                    if k == p or k.startswith(p + "/"):
                        rest = k[len(p):]
                        nk = np_ + "/".join(nfc(x) for x in rest.split("/"))
                        del new[k]
                        new[nk] = t[k]
                        if k != p:
                            if disk_kind(b, nk) is not None and nk not in t: raise Refuse("untracked inside dest")
            emptied = [d for d in dirs_of(t) - dirs_of(new) if not any(d == p or d.startswith(p + "/") for p in srcs)]
            if emptied: raise Either("emptied dir")
        else:
            raise Either("?")
        req, either, forbid = keep_rules(new, t, b, created)
        for d in created:
            if d in forbid: raise Either("created dir w/o keep")
        return ("accept", apply_keeps(new, req), either, forbid)
    except Refuse as e:
        return ("refuse", str(e), None, None)
    except Either as e:
        return ("either", str(e), None, None)

def _folded_exists(t, comps):
    cur = "corporate"
    for c in comps:
        ch = children(t, cur)
        m = [n for n in ch if nfc(n) == nfc(c)]
        if m: cur = cur + "/" + m[0]; continue
        return any(fold(n) == fold(c) for n in ch)
    return False

def _disk_names(b, d):
    try: return os.listdir(os.path.join(b, d))
    except Exception: return []

def compare(exp, got, either, forbid):
    """exp, got: corporate trees. ADD mode means 100644 or 100755"""
    diffs = []
    def keepdir(p):
        return p.rsplit("/", 1)[0] if iskeep(p.rsplit("/", 1)[1]) else None
    for p in set(exp) | set(got):
        e, g = exp.get(p), got.get(p)
        if e == g: continue
        if e and g and e[0] == "ADD" and g[0] in ("100644", "100755") and e[1] == g[1]: continue
        kd = keepdir(p)
        if kd and kd in either and (e is None or g is None): continue
        diffs.append((p, e, g))
    return sorted(diffs, key=lambda x: x[0])

# ---------- generation ----------
GOOD = ["a", "b", "Minutes 2024.pdf", nfc("ä.txt"), "notes~", "[a]", "*", "Foo", "foo", "ß", "ss",
        "Σ.txt", "σ.txt", "deep", "AGM", "x y", "reg.pdf", "c", "d"]
BADARG = ["..", ".git", ".hidden", "trail.", "trail ", "a\\b", ".keep", ".Keep"]

class Seq:
    def __init__(self, base, seed):
        self.r = random.Random(seed)
        self.seed = seed
        self.base = base
        self.b = os.path.join(base, "books")
        self.src = os.path.join(base, "src")
        os.makedirs(self.b); os.makedirs(self.src)
        rc, o, e = sh([ABO, "init", "Test", "LLC", "--currency", "EUR", "--vat", "none", "--git-name", "t", "--git-email", "t@x.invalid"], self.b)
        assert rc == 0, e
        self.hand = False
        self.abo_commits = {git(self.b, "rev-parse", "HEAD")[1].strip(): "init"}
        self.models = {}
        self.models[git(self.b, "rev-parse", "HEAD")[1].strip()] = {}
        self.model = {}
        self.log = []
        self.issues = []
        self.counts = dict(steps=0, ok=0, refused=0, injected=0, inj_failed=0, undo=0, hand=0, drops=0)
        self.nsrc = 0
        self.accts = 0
        self.handy = (seed % 2 == 0)

    # random paths
    def existing(self, files=True, dirs=True):
        t = self.model
        c = []
        if files: c += [p for p in t if not iskeep(p.rsplit("/", 1)[1])]
        if dirs: c += sorted(dirs_of(t) - {"corporate"})
        return sorted(c)

    def rand_dir(self):
        r = self.r
        ds = sorted(dirs_of(self.model) - {"corporate"}) or ["corporate"]
        if r.random() < 0.55 and ds:
            d = r.choice(ds + ["corporate"])
        else:
            d = "corporate"
        for _ in range(r.choice([0, 1, 1, 2])):
            d += "/" + r.choice(GOOD)
        if r.random() < 0.04: d += "/" + r.choice(BADARG)
        if r.random() < 0.03: d = r.choice(["/etc", "corporate/../x", "journal", "corporate/a//b"])
        if r.random() < 0.05: d = nfd(d)
        return d if d != "corporate" or r.random() < 0.2 else d + "/" + r.choice(GOOD)

    def rand_path(self):
        r = self.r
        ex = self.existing()
        if ex and r.random() < 0.8:
            p = r.choice(ex)
            if r.random() < 0.05: p = p.upper().replace("CORPORATE", "corporate")
            if r.random() < 0.05: p = nfd(p)
            return p
        if r.random() < 0.1: return r.choice(["corporate", "corporate/", "corporate/..", "/tmp", "corporate/.git"])
        ks = [p for p in self.model if iskeep(p.rsplit("/", 1)[1])]
        if ks and r.random() < 0.3: return r.choice(ks)
        return self.rand_dir()

    def make_src(self):
        r = self.r
        self.nsrc += 1
        d = os.path.join(self.src, "s%d" % self.nsrc); os.makedirs(d)
        n = r.choice(GOOD) if r.random() < 0.95 else r.choice([".hidden", "trail.", "x\\y"])
        data = os.urandom(r.choice([0, 3, 50, 2000])) if r.random() < 0.5 else ("doc %d\n" % self.nsrc).encode()
        if r.random() < 0.2:
            tgt = os.path.join(d, "target-" + str(self.nsrc))
            with open(tgt, "wb") as f: f.write(data)
            os.symlink("target-" + str(self.nsrc), os.path.join(d, n))
        else:
            with open(os.path.join(d, n), "wb") as f: f.write(data)
            if r.random() < 0.3: os.chmod(os.path.join(d, n), 0o755)
        return os.path.relpath(os.path.join(d, n), self.src)

    def gen(self):
        r = self.r
        x = r.random()
        if x < 0.30:
            k = r.choice([1, 1, 2, 3])
            srcs = [self.make_src() for _ in range(k)]
            if r.random() < 0.05: srcs.append(srcs[0])
            return ("add", srcs, self.rand_dir())
        if x < 0.40: return ("mkdir", self.rand_dir())
        if x < 0.58:
            srcs = [self.rand_path() for _ in range(r.choice([1, 1, 1, 2, 3, 4]))]
            to = self.rand_dir() if r.random() < 0.5 else (self.rand_path().rsplit("/", 1)[0] + "/" + r.choice(GOOD))
            if r.random() < 0.3: to += "/"
            if r.random() < 0.08 and len(srcs) == 1 and "/" in srcs[0]:
                # rename in other capitals
                d, n = srcs[0].rsplit("/", 1); to = d + "/" + n.swapcase()
            return ("mv", srcs, to)
        if x < 0.70:
            return ("rm", [self.rand_path() for _ in range(r.choice([1, 1, 2, 3, 4]))])
        if x < 0.82: return ("undo", r.choice([1, 1, 1, 2, 3]))
        if x < 0.88 or not self.handy:
            if x >= 0.88: return ("undo", 1)
            self.accts += 1
            if r.random() < 0.5: return ("acct", ["account", "add", "Expenses:Foo" + "abcdefghijklmnopqrstuvwxyz"[self.accts % 26] + str(self.seed % 7)])
            return ("acct", ["post", "--desc", "d%d" % self.accts, "Expenses:General 5", "Assets:Bank"])
        return ("hand",)

    def argv(self, op):
        if op[0] == "add": return ["corporate", "add"] + [os.path.join(self.src, s) for s in op[1]] + [op[2]]
        if op[0] == "mkdir": return ["corporate", "mkdir", op[1]]
        if op[0] == "mv": return ["corporate", "mv"] + op[1] + [op[2]]
        if op[0] == "rm": return ["corporate", "rm"] + op[1]
        if op[0] == "undo": return ["undo"]
        if op[0] == "acct": return op[1]

    # hand commits
    def hand_commit(self):
        r = self.r; b = self.b
        ds = sorted(dirs_of(self.model) - {"corporate"})
        base = r.choice(ds) if ds and r.random() < 0.6 else "corporate"
        kind_ = r.choice(["odd", "exec", "link", "nokeep", "twin", "baddir", "Keep", "twinfile", "nfd"])
        paths = []
        def wr(rel, data=b"hand\n", mode=0o644):
            full = os.path.join(b, rel)
            os.makedirs(os.path.dirname(full), exist_ok=True)
            if os.path.lexists(full): return
            with open(full, "wb") as f: f.write(data)
            os.chmod(full, mode); paths.append(rel)
        if kind_ == "odd": wr(base + "/" + r.choice([".hidden", "trail.", "trail ", "a b", "x*", "[a]"]))
        elif kind_ == "exec": wr(base + "/run%d.sh" % r.randrange(9), b"#!/bin/sh\n", 0o755)
        elif kind_ == "link":
            rel = base + "/ln%d" % r.randrange(9)
            full = os.path.join(b, rel); os.makedirs(os.path.dirname(full), exist_ok=True)
            if not os.path.lexists(full): os.symlink(r.choice(["../abo.toml", "nowhere", "/etc/passwd"]), full); paths.append(rel)
        elif kind_ == "nokeep": wr("corporate/hand%d/sub/f.txt" % r.randrange(5))
        elif kind_ == "twin":
            if ds:
                d = r.choice(ds); par, n = d.rsplit("/", 1)
                tw = n.swapcase() if n.swapcase() != n else n + "X"
                wr(par + "/" + tw + "/t.txt")
            else: wr("corporate/T/t.txt"); wr("corporate/t/t.txt")
        elif kind_ == "baddir": wr("corporate/" + r.choice([".hd", "dot.", "sp "]) + "/f.txt")
        elif kind_ == "Keep": wr("corporate/kk%d/.Keep" % r.randrange(5), b""); wr(base + "/.KEEP", b"")
        elif kind_ == "twinfile": wr(base + "/Twin.txt"); wr(base + "/twin.txt", b"other\n")
        elif kind_ == "nfd": wr(nfd(base + "/" + "dä") + "/" + nfd("ü.txt"))
        if not paths: return False
        rc, o, e = git(b, "add", "-f", "--", *paths)
        if rc: self.issues.append(("harness", "hand add failed", e)); return False
        git(b, "commit", "-q", "--no-verify", "-m", "hand " + kind_)
        self.model = ctree(tree(b))
        self.models[git(b, "rev-parse", "HEAD")[1].strip()] = dict(self.model)
        self.hand = True; self.counts["hand"] += 1
        self.log.append(("hand", kind_, paths))
        return True

    # drops
    def drop(self, op):
        r = self.r; b = self.b
        cands = []
        if op[0] == "add":
            n = nfc(os.path.basename(op[1][0]))
            cands.append(op[2].rstrip("/") + "/" + n)
            cands.append(op[2].rstrip("/") + "/.keep")
        if op[0] == "mv":
            to = op[2].rstrip("/")
            cands.append(to)
            if op[1]: cands.append(to + "/" + op[1][0].rsplit("/", 1)[-1])
        if op[0] == "mkdir": cands.append(op[1].rstrip("/") + "/.keep"); cands.append(op[1])
        if op[0] == "undo":
            rc, o, e = git(b, "diff", "--name-only", "--no-renames", "-z", "HEAD", "HEAD^")
            cands += [p for p in o.split("\0") if p.startswith("corporate/")]
        ds = sorted(dirs_of(self.model) - {"corporate"})
        if ds:
            cands.append(r.choice(ds) + "/.keep")
            cands.append(r.choice(ds) + "/" + r.choice(GOOD))
        cands = [c for c in cands if c.startswith("corporate/") and ".." not in c.split("/") and "//" not in c and ".git" not in c.split("/")]
        if not cands: return
        p = r.choice(cands)
        if r.random() < 0.3: p = p + "~"
        full = os.path.join(b, p)
        if os.path.lexists(full): return
        try:
            os.makedirs(os.path.dirname(full), exist_ok=True)
            if r.random() < 0.15: os.symlink("nowhere", full)
            elif r.random() < 0.1: os.makedirs(full); open(os.path.join(full, "inner"), "w").write("u")
            else:
                with open(full, "w") as f: f.write("untracked %s\n" % p)
        except (NotADirectoryError, FileExistsError, PermissionError, OSError):
            return
        self.counts["drops"] += 1
        self.log.append(("drop", p))

    # injections
    def inject(self, op):
        r = self.r; b = self.b
        opts = ["hook", "lock", "abo_ro", "chmod"]
        if op[0] == "add": opts.append("ulimit"); opts.append("ulimit")
        kind_ = r.choice(opts)
        undo = None
        if kind_ == "hook":
            h = os.path.join(b, ".git/hooks/pre-commit")
            with open(h, "w") as f: f.write("#!/bin/sh\nexit 1\n")
            os.chmod(h, 0o755); undo = lambda: os.remove(h)
        elif kind_ == "lock":
            l = os.path.join(b, ".git/index.lock"); open(l, "w").close(); undo = lambda: os.path.exists(l) and os.remove(l)
        elif kind_ == "abo_ro":
            a = os.path.join(b, ".abo"); os.makedirs(a, exist_ok=True); m = os.stat(a).st_mode
            os.chmod(a, 0o555); undo = lambda: os.chmod(a, stat.S_IMODE(m))
        elif kind_ == "chmod":
            cands = []
            for a in ([op[2]] if op[0] in ("add", "mv") else []) + (op[1] if op[0] in ("mv", "rm") else []) + ([op[1]] if op[0] == "mkdir" else []):
                if isinstance(a, str) and a.startswith("corporate"):
                    q = a.rstrip("/")
                    while q:
                        if os.path.isdir(os.path.join(b, q)) and not os.path.islink(os.path.join(b, q)): cands.append(q); break
                        q = q.rsplit("/", 1)[0] if "/" in q else ""
                        if q and os.path.isdir(os.path.join(b, q)) and not os.path.islink(os.path.join(b, q)): cands.append(q); break
            if op[0] == "add": cands.append(os.path.join(self.src, op[1][0]).rsplit("/", 1)[0])
            if op[0] == "undo": cands += sorted(dirs_of(self.model))
            if not cands: cands = ["corporate"] if os.path.isdir(os.path.join(b, "corporate")) else []
            if not cands: return None, None
            d = r.choice(cands); full = d if d.startswith("/") else os.path.join(b, d)
            m = os.stat(full).st_mode; os.chmod(full, 0o555); undo = lambda: os.chmod(full, stat.S_IMODE(m))
            kind_ = "chmod:" + d
        elif kind_ == "ulimit":
            # make the first source big
            sp = os.path.join(self.src, op[1][0])
            if os.path.islink(sp): sp = os.path.realpath(sp)
            with open(sp, "wb") as f: f.write(os.urandom(300 * 1024))
            return "ulimit", (lambda: None)
        return kind_, undo

    def check_ok(self):
        rc, o, e = sh([ABO, "check"], self.b)
        return rc, o + e

    def step(self, op, inj):
        b = self.b
        argv = self.argv(op)
        pre = snap(b)
        t_pre = tree(b); c_pre = ctree(t_pre)
        if c_pre != self.model:
            self.issues.append(("harness-model-drift", op, sorted(set(c_pre.items()) ^ set(self.model.items()))[:5]))
            self.model = c_pre
        unt_pre = untracked(b, pre["files"], t_pre)
        if op[0] == "undo":
            head = pre["head"]
            if head in self.abo_commits and self.abo_commits[head] != "init":
                par = git(b, "rev-parse", "HEAD^")[1].strip()
                exp = self.models.get(par)
                if exp is None: m = ("either", "no model", None, None)
                else:
                    # untracked in the way?
                    blocked = False
                    for p in set(exp) - set(self.model):
                        q = p
                        if disk_kind(b, p) is not None: blocked = True
                        parts = p.split("/")
                        for i in range(2, len(parts)):
                            if disk_kind(b, "/".join(parts[:i])) in ("f", "l"): blocked = True
                    m = ("refuse", "blocked", None, None) if blocked else ("accept", exp, set(), set())
            elif head in self.abo_commits:
                m = ("either", "undo init", None, None)
            else:
                m = ("refuse", "hand commit last", None, None)
        elif op[0] == "acct":
            m = ("either", "acct", None, None)
        else:
            m = model(op, self.model, b, dict(src=self.src))
        if inj == "ulimit":
            cmd = "trap '' XFSZ; ulimit -f 64; exec " + " ".join(shlex.quote(a) for a in [ABO] + argv)
            rc, o, e = sh(["bash", "-c", cmd], b)
        else:
            rc, o, e = sh([ABO] + argv, b)
        self.counts["steps"] += 1
        return pre, t_pre, unt_pre, m, rc, o, e, argv

    def verify(self, op, inj, pre, t_pre, unt_pre, m, rc, o, e, argv):
        b = self.b
        post = snap(b)
        tag = (op, inj)
        if "panicked" in e: self.issues.append(("panic", argv, inj, e[-500:]))
        if rc != 0:
            self.counts["refused"] += 1
            if inj: self.counts["inj_failed"] += 1
            # git's index held by another: abo leaves the change for the next
            # abo to take back (MODEL.md §5), which is what is checked.
            if inj == "lock":
                sh([ABO, "check"], b); post = snap(b)
            for k in ("head", "index"):
                if pre[k] != post[k]: self.issues.append(("failed-but-changed-" + k, argv, inj, e[-300:]))
            if pre["files"] != post["files"]:
                d = [(p, pre["files"].get(p), post["files"].get(p)) for p in sorted(set(pre["files"]) | set(post["files"])) if pre["files"].get(p) != post["files"].get(p)]
                self.issues.append(("failed-but-changed-files", argv, inj, d[:6], e[-300:]))
            aside = os.path.join(b, ".abo/aside")
            if os.path.isdir(aside) and os.listdir(aside):
                self.issues.append(("aside-not-empty", argv, inj, os.listdir(aside)[:5]))
            crc, co = self.check_ok()
            if "abo did not make" in co:
                self.issues.append(("check-reports-foreign-change-after-failure", argv, inj, co[:400]))
            if not inj and m[0] == "accept":
                self.issues.append(("model-accepts-abo-refuses", argv, e.strip()[-400:]))
            return False
        # success
        self.counts["ok"] += 1
        heads = git(b, "rev-list", "--parents", "-n", "1", "HEAD")[1].split()
        if post["head"] == pre["head"]:
            self.issues.append(("success-no-commit", argv, inj, o[-200:]))
        elif heads[1:] != [pre["head"]]:
            self.issues.append(("not-one-commit", argv, inj))
        t_post = tree(b); c_post = ctree(t_post)
        changed = sorted(p for p in set(t_pre) | set(t_post) if t_pre.get(p) != t_post.get(p))
        if op[0] in ("add", "mkdir", "mv", "rm"):
            bad = [p for p in changed if not p.startswith("corporate/")]
            if bad: self.issues.append(("changed-outside-corporate", argv, bad))
        if op[0] == "acct":
            bad = [p for p in changed if p.startswith("corporate/")]
            if bad: self.issues.append(("acct-changed-corporate", argv, bad))
        unt_post = untracked(b, post["files"], t_post)
        if unt_pre != unt_post:
            d = [(p, unt_pre.get(p), unt_post.get(p)) for p in sorted(set(unt_pre) | set(unt_post)) if unt_pre.get(p) != unt_post.get(p)]
            self.issues.append(("untracked-changed", argv, inj, d[:6]))
        st = git(b, "status", "--porcelain", "--untracked-files=no")[1]
        if st.strip(): self.issues.append(("worktree-not-clean", argv, st[:300]))
        crc, co = self.check_ok()
        if crc != 0: self.issues.append(("check-fails-after-success", argv, inj, co[:400]))
        if not inj and m[0] == "refuse":
            self.issues.append(("model-refuses-abo-accepts", argv, m[1], changed[:8]))
        elif m[0] == "accept":
            diffs = compare(m[1], c_post, m[2], m[3])
            if diffs: self.issues.append(("model-differs", argv, inj, diffs[:8]))
        # empty directories left on disk under corporate with nothing in them
        for p, v in post["files"].items():
            if v[0] == "d" and p.startswith("corporate") and not os.listdir(os.path.join(b, p)) and p not in pre["files"]:
                self.issues.append(("empty-dir-left", argv, p))
        self.model = c_post
        h = post["head"]
        self.abo_commits[h] = op[0]
        self.models[h] = dict(c_post)
        return True

    def run(self, nsteps):
        r = self.r
        for i in range(nsteps):
            op = self.gen()
            if op[0] == "hand":
                self.hand_commit(); continue
            reps = op[1] if op[0] == "undo" else 1
            if op[0] == "undo": op = ("undo",)
            for _ in range(reps):
                if r.random() < 0.25: self.drop(op)
                inj, undo_inj = (None, None)
                if r.random() < 0.3:
                    inj, undo_inj = self.inject(op)
                    if inj: self.counts["injected"] += 1
                try:
                    res = self.step(op, inj)
                finally:
                    if undo_inj: undo_inj()
                if op[0] == "undo": self.counts["undo"] += 1
                # verify after restoring permissions (snapshot unaffected by perms except chmod'd dir mode)
                if inj and inj.startswith("chmod"):
                    # pre-snapshot was taken with 555; re-take mode in post: compare by temporarily re-applying
                    pass
                self.log.append((op, inj, res[4], res[6].strip()[-160:]))
                self.verify_wrap(op, inj, res)

    def verify_wrap(self, op, inj, res):
        pre, t_pre, unt_pre, m, rc, o, e, argv = res
        if inj and inj.startswith("chmod:"):
            # pre has the 555 mode on the chmodded path; normalise both
            d = inj[6:]
            if not d.startswith("/"):
                post_mode = None
                if d in pre["files"]:
                    full = os.path.join(self.b, d)
                    if os.path.isdir(full):
                        pre["files"][d] = ("d", stat.S_IMODE(os.stat(full).st_mode))
        self.verify(op, inj, pre, t_pre, unt_pre, m, rc, o, e, argv)

    def replay(self):
        b = self.b
        rc, o, e = sh([ABO, "log", "--json", "--last", "100000"], b)
        lines = json.loads(o)
        again = os.path.join(self.base, "again"); os.makedirs(again)
        docs = os.path.join(self.base, "rdocs"); os.makedirs(docs)
        for k, l in enumerate(lines):
            argv = list(l["argv"])
            if argv[:2] == ["corporate", "add"]:
                start = 3 if argv[2] == "--" else 2
                for i in range(start, len(argv) - 1):
                    p = argv[i]
                    rc2, data, _ = None, subprocess.run(["git", "show", "%s:%s" % (l["commit"], p)], cwd=b, capture_output=True).stdout, None
                    d = os.path.join(docs, "%d-%d" % (k, i)); os.makedirs(d)
                    f = os.path.join(d, p.rsplit("/", 1)[1])
                    with open(f, "wb") as fh: fh.write(data)
                    argv[i] = f
            rc, o2, e2 = sh([ABO] + argv, again)
            if rc != 0:
                self.issues.append(("replay-failed", argv, e2[-300:])); return
        ta, tb = tree(again), tree(b)
        if ta != tb:
            d = [(p, tb.get(p), ta.get(p)) for p in sorted(set(ta) | set(tb)) if ta.get(p) != tb.get(p)]
            self.issues.append(("replay-differs", d[:8]))

def main():
    base = os.path.abspath(sys.argv[2]); s0, s1 = int(sys.argv[3]), int(sys.argv[4])
    bad = 0
    out = open(os.path.join(base, "res-%d.jsonl" % s0), "w")
    for seed in range(s0, s1):
        d = os.path.join(base, "w", "s%d" % seed)
        try:
            sq = Seq(d, seed)
            n = sq.r.randint(10, 30)
            sq.run(n)
            if not sq.hand: sq.replay()
            rec = dict(seed=seed, counts=sq.counts, hand=sq.hand, issues=sq.issues, log=sq.log)
        except Exception as ex:
            rec = dict(seed=seed, crash=traceback.format_exc())
        out.write(json.dumps(rec, default=str) + "\n"); out.flush()
        if rec.get("issues") or rec.get("crash"):
            bad += 1
            print(json.dumps(rec, default=str, ensure_ascii=False)[:4000])
        # restore perms and clean up on no issues
        subprocess.run(["chmod", "-R", "u+rwx", d], capture_output=True)
        if not rec.get("issues") and not rec.get("crash"):
            shutil.rmtree(d, ignore_errors=True)

    print(f"seeds {s0} to {s1 - 1}: {bad} with issues")
    sys.exit(1 if bad else 0)

if __name__ == "__main__":
    main()
