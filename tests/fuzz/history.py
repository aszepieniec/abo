#!/usr/bin/env python3
"""Long mixed histories: bookkeeping + corporate, with undo/redo/tidy/check.
Model of the corporate tree kept here; replay of the log into fresh books.

Usage: history.py ABO WORKDIR SEED STEPS. Exits 1 on any issue, printing
each. tests/fuzz.rs runs it; ABO_FUZZ=n runs n times as many."""
import os, sys, json, random, subprocess, shutil, unicodedata, zlib, struct, hashlib

ABO = os.path.abspath(sys.argv[1])
HERE = os.path.abspath(sys.argv[2])
ENV = dict(os.environ, ABO_TODAY='2025-12-01', GIT_AUTHOR_NAME='t', GIT_AUTHOR_EMAIL='t@x.invalid',
           GIT_COMMITTER_NAME='t', GIT_COMMITTER_EMAIL='t@x.invalid')

def nfc(s): return unicodedata.normalize('NFC', s)

# The directories new books open with, taken out at once: the model starts
# from nothing, and the log holds it, as replay needs.
def bare(abo, books, env=None):
    import json as _j
    p = subprocess.run([abo, "--json", "corporate", "list"], cwd=books, env=env, capture_output=True, text=True)
    dirs = [d for d in _j.loads(p.stdout)["directories"] if d.count("/") == 1]
    if dirs:
        p = subprocess.run([abo, "corporate", "rm", "--"] + dirs, cwd=books, env=env, capture_output=True, text=True)
        assert p.returncode == 0, p.stderr
def fold(s): return nfc(nfc(s).casefold())

def run(cwd, args, env=ENV, check=False):
    p = subprocess.run([ABO] + args, cwd=cwd, env=env, capture_output=True, text=True)
    return p

def git(cwd, *a):
    return subprocess.run(['git', '-c', 'core.quotepath=off'] + list(a), cwd=cwd, capture_output=True, text=True).stdout

def head(cwd): return git(cwd, 'rev-parse', 'HEAD').strip()

def png(tag):
    def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 1, 1, 8, 0, 0, 0, 0)) +
            chunk(b'tEXt', b'k\x00' + tag.encode()) + chunk(b'IDAT', zlib.compress(b'\x00\x00')) + chunk(b'IEND', b''))

NAMES0 = ['Zé', 'Zé', 'zé', 'ZÉ', 'Straße', 'STRASSE', 'strasse', 'Σίσυφος', 'σίσυφοσ', 'ΣΊΣΥΦΟΣ',
         'a b', 'A B', 'Ünïcödé', 'Ünïcödé', 'minutes', 'Minutes', '2025', 'statutes',
         'x y  z', 'Ångström', 'Ångström', 'ﬁle', 'FILE', 'résumé', 'RÉSUMÉ', 'd']

NAMES = NAMES0 if not os.environ.get('FEW') else ['Zé', 'Ze\u0301', 'ZÉ', 'Straße', 'STRASSE', 'a b', 'A B', 'σς', 'ΣΣ']
class Model:
    def __init__(s): s.files = {}; s.dirs = set()  # NFC paths; files: path->sha
    def copy(s):
        m = Model(); m.files = dict(s.files); m.dirs = set(s.dirs); return m
    def entries(s, d):  # children names of dir d (d '' == corporate)
        out = {}
        for p in list(s.files) + list(s.dirs):
            par, _, base = p.rpartition('/')
            if par == d: out[base] = p
        return out
    def isdir(s, p): return p == 'corporate' or p in s.dirs
    def exists(s, p): return p in s.files or s.isdir(p)
    def taken(s, d, name, exclude=()):
        for b, p in s.entries(d).items():
            if p in exclude: continue
            if fold(b) == fold(name): return p
        return None
    def ensure_dir(s, d):
        """Make d and parents; return False if a file is in the way or a fold-twin."""
        parts = d.split('/'); cur = parts[0]
        for name in parts[1:]:
            nxt = cur + '/' + name
            if nxt in s.files: return False
            if nxt not in s.dirs:
                if s.taken(cur, name): return False
                s.dirs.add(nxt)
            cur = nxt
        return True
    def subtree(s, p):
        return [x for x in list(s.files) + list(s.dirs) if x == p or x.startswith(p + '/')]

def actual(books):
    out = git(books, 'ls-files', '-z', 'corporate')
    files = {}; dirs = set(); keeps = set()
    for p in out.split('\0'):
        if not p: continue
        par, _, base = p.rpartition('/')
        if base == '.keep': keeps.add(par); continue
        files[p] = hashlib.sha1(open(os.path.join(books, p), 'rb').read()).hexdigest()
        # parents
    for p in list(files) + [k for k in keeps]:
        parts = p.split('/')
        for i in range(2, len(parts) + (1 if p in keeps else 0)):
            dirs.add('/'.join(parts[:i]))
    dirs |= {k for k in keeps if k != 'corporate'}
    dirs.discard('corporate')
    return files, dirs, keeps

class Fuzz:
    def __init__(s, seed, n):
        s.r = random.Random(seed); s.n = n; s.seed = seed
        s.root = os.path.join(HERE, f'run{seed}'); shutil.rmtree(s.root, ignore_errors=True)
        s.books = os.path.join(s.root, 'books'); s.src = os.path.join(s.root, 'src')
        os.makedirs(s.books); os.makedirs(s.src)
        p = run(s.books, ['init', 'Test LLC', '--currency', 'EUR', '--git-name', 't', '--git-email', 't@x.invalid', '--ni'])
        assert p.returncode == 0, p.stderr
        bare(ABO, s.books, ENV)
        s.m = Model(); s.before = None; s.last_invoice = False; s.nfile = 0; s.partners = []
        s.issues = []; s.trace = []

    def rp(s):
        """random corporate path in the model, or a new one"""
        allp = list(s.m.files) + sorted(s.m.dirs)
        if allp and s.r.random() < 0.6: return s.r.choice(allp)
        base = s.r.choice(['corporate'] + sorted(s.m.dirs)) if s.m.dirs and s.r.random() < 0.7 else 'corporate'
        for _ in range(s.r.choice([1, 1, 2, 3])): base += '/' + s.r.choice(NAMES)
        return base
    def rdir(s):
        if s.m.dirs and s.r.random() < 0.6: return s.r.choice(sorted(s.m.dirs))
        return s.rp()

    def note(s, msg):
        s.issues.append(msg); print(f'[seed {s.seed}] {msg}', flush=True)

    def do(s, args, predict, newmodel=None, kind='op'):
        h0 = head(s.books)
        p = run(s.books, args)
        ok = p.returncode == 0
        h1 = head(s.books)
        s.trace.append((args, ok, p.stderr.strip()[:200]))
        if not ok and h0 != h1: s.note(f'refused but committed: {args}: {p.stderr}')
        s.committed = ok and h0 != h1
        if ok and h0 == h1 and args[0] != 'config': s.note(f'taken without commit: {args}')
        if predict is not None and ok != predict:
            s.note(f'model {"takes" if predict else "refuses"}, abo {"took" if ok else "refused"}: {args!r}: {p.stderr.strip()[:300]}')
        return ok, p

    def step(s):
        r = s.r.random()
        if os.environ.get('FOCUS'):
            r = s.r.random() * 0.6
        if r < 0.12: s.c_add()
        elif r < 0.20: s.c_mkdir()
        elif r < 0.36: s.c_mv()
        elif r < 0.44: s.c_rm()
        elif r < 0.56: s.undo()
        elif r < 0.60: s.tidy()
        else: s.book()

    def commit_model(s, ok, m2):
        if ok and s.committed:
            s.before = s.m; s.m = m2 if m2 is not None else s.m.copy(); s.last_invoice = False

    # ---- corporate ----
    def c_add(s):
        k = s.r.choice([1, 1, 2, 3])
        d = s.rdir() if s.r.random() < 0.8 else s.rp()
        if s.r.random() < 0.2: d = unicodedata.normalize('NFD', d)
        dn = nfc(d)
        srcs = []
        for i in range(k):
            name = s.r.choice(NAMES) + s.r.choice(['', '.pdf', '.txt'])
            sd = os.path.join(s.src, str(s.nfile)); os.makedirs(sd); s.nfile += 1
            f = os.path.join(sd, name); open(f, 'wb').write(f'{s.nfile} {name}'.encode())
            srcs.append(f)
        m2 = s.m.copy(); pred = True
        if dn in m2.files: pred = False
        elif not m2.ensure_dir(dn): pred = False
        else:
            seen = []
            for f in srcs:
                b = nfc(os.path.basename(f))
                if m2.taken(dn, b) or any(fold(x) == fold(b) for x in seen): pred = False; break
                seen.append(b)
                m2.files[dn + '/' + b] = hashlib.sha1(open(f, 'rb').read()).hexdigest()
        ok, _ = s.do(['corporate', 'add'] + srcs + [d], pred)
        s.commit_model(ok, m2 if pred else None)
        if ok and not pred: s.resync()

    def c_mkdir(s):
        d = s.rp()
        if s.r.random() < 0.5: d = d + '/' + s.r.choice(NAMES)
        m2 = s.m.copy()
        if s.r.random() < 0.2: d = unicodedata.normalize('NFD', d)
        pred = not m2.exists(nfc(d)) and m2.ensure_dir(nfc(d))
        par, _, base = d.rpartition('/')
        ok, _ = s.do(['corporate', 'mkdir', d], pred)
        s.commit_model(ok, m2 if pred else None)
        if ok and not pred: s.resync()

    def c_mv(s):
        k = s.r.choice([1, 1, 1, 2, 3])
        froms = [s.rp() for _ in range(k)]
        if s.r.random() < 0.15: froms[0] = unicodedata.normalize('NFD', froms[0])
        to = s.rdir() if s.r.random() < 0.6 else s.rp()
        if s.r.random() < 0.15: to += '/'
        if s.r.random() < 0.15 and len(froms) == 1 and froms[0] in s.m.dirs:  # into itself / own child
            to = froms[0] + '/' + s.r.choice(NAMES)
        if s.r.random() < 0.1 and len(froms) == 1:  # case rename
            par, _, b = nfc(froms[0]).rpartition('/'); to = par + '/' + s.r.choice([b.upper(), b.lower(), b.swapcase()])
        pred, m2 = s.mv_model(froms, to)
        ok, p = s.do(['corporate', 'mv'] + froms + [to], pred)
        s.commit_model(ok, m2 if pred else None)
        if ok and not pred: s.resync()

    def mv_model(s, froms, to):
        m2 = s.m.copy()
        fr = [nfc(f).rstrip('/') for f in froms]; to_n = nfc(to)
        if len(set(fr)) != len(fr): return False, None
        for f in fr:
            if f == 'corporate' or not m2.exists(f): return False, None
            if any(g != f and f.startswith(g + '/') for g in fr): return False, None
        into = to_n.endswith('/') or len(fr) > 1 or m2.isdir(to_n.rstrip('/'))
        tdir = to_n.rstrip('/')
        if into:
            if tdir in m2.files: return False, None
            dests = [(f, tdir + '/' + f.rpartition('/')[2]) for f in fr]
        else:
            dests = [(fr[0], tdir)]
        for f, dst in dests:
            if dst == f: return False, None
            if dst.startswith(f + '/'): return False, None
        # names
        dnames = [fold(dst) for _, dst in dests]
        if len(set(dnames)) != len(dnames): return False, None
        moved = set()
        for f, _ in dests: moved |= set(m2.subtree(f))
        for f, dst in dests:
            par, _, b = dst.rpartition('/')
            # parent must be a dir or makeable
            if par in m2.files: return False, None
            # fold-twin at dst, excluding the moved thing itself when it's a case-rename in same dir
            t = m2.taken(par, b, exclude=moved) if m2.isdir(par) else None
            if t: return False, None
        # apply
        for f, dst in dests:
            sub = m2.subtree(f)
            par = dst.rpartition('/')[0]
            if not m2.ensure_dir(par) if par != 'corporate' else False: return False, None
            newfiles = {}; newdirs = set()
            for x in sub:
                y = dst + x[len(f):]
                if x in m2.files: newfiles[y] = m2.files.pop(x)
                else: m2.dirs.discard(x); newdirs.add(y)
            m2.files.update(newfiles); m2.dirs |= newdirs
        return True, m2

    def c_rm(s):
        k = s.r.choice([1, 1, 2, 3, 4])
        ps = [s.rp() for _ in range(k)]
        if s.r.random() < 0.1: ps = [unicodedata.normalize('NFD', p) for p in ps]
        m2 = s.m.copy(); pred = True; n = [nfc(p) for p in ps]
        if len(set(n)) != len(n): pred = False
        for p in n:
            if p == 'corporate' or not m2.exists(p): pred = False
        if pred:
            for p in n:
                for x in m2.subtree(p):
                    m2.files.pop(x, None); m2.dirs.discard(x)
        ok, _ = s.do(['corporate', 'rm'] + ps, pred)
        s.commit_model(ok, m2 if pred else None)
        if ok and not pred: s.resync()

    def resync(s):
        f, d, _ = actual(s.books); s.m.files = f; s.m.dirs = d

    # ---- undo/tidy/check ----
    def undo(s):
        pred = s.before is not None and not s.last_invoice
        ok, p = s.do(['undo'], None)
        if ok:
            if s.before is None: s.note('undo taken with nothing in model to undo')
            else: s.m, s.before = s.before, s.m
        elif pred:
            # bookkeeping undo might legitimately be refused? record
            s.trace[-1] = s.trace[-1] + ('undo-refused',)

    def tidy(s):
        h0 = head(s.books); p = run(s.books, ['tidy'])
        if head(s.books) != h0: s.note(f'tidy committed something: {git(s.books, "show", "--stat", "HEAD")}')
        s.trace.append((['tidy'], p.returncode == 0, p.stderr[:200]))

    # ---- bookkeeping ----
    def book(s):
        r = s.r.random(); m = s.r.randint(1, 11); dd = s.r.randint(1, 28); date = f'2025-{m:02d}-{dd:02d}'
        amt = f'{s.r.randint(1, 500)}.{s.r.randint(0, 99):02d}'
        if r < 0.25:
            ok, _ = s.do(['post', '--date', date, '--desc', s.r.choice(['Pens', 'Rent é', 'Zé Café']), f'Expenses:General {amt}', 'Assets:Bank'], True)
            s.commit_model(ok, None)
        elif r < 0.33:
            pid = f'p{len(s.partners)}'
            ok, _ = s.do(['partner', 'add', pid, '--name', s.r.choice(['Ünï AG', 'Acme', 'Zé Ltd'])], True)
            if ok: s.partners.append(pid)
            s.commit_model(ok, None)
        elif r < 0.50:
            s.nfile += 1; f = os.path.join(s.src, f'doc{s.nfile}.png'); open(f, 'wb').write(png(f'{s.seed}-{s.nfile}'))
            ok, p = s.do(['file', f, '--date', date, '--issuer', s.r.choice(['Acme Ltd', 'Zé Café'])], True)
            s.commit_model(ok, None)
            if ok and s.r.random() < 0.7:
                b = json.loads(run(s.books, ['--json', 'bills']).stdout)['waiting']
                if b:
                    a = ['bill', '--document', b[-1], '--desc', 'Hosting', f'Expenses:General {amt}', 'Liabilities:Payable']
                    if s.partners and s.r.random() < 0.5: a += ['--partner', s.r.choice(s.partners)]
                    ok, _ = s.do(a, None); s.commit_model(ok, None)
        elif r < 0.62:
            ok, _ = s.do(['invoice', '--to', 'Foo é', '--item', f'1|{amt}|Work', '--date', '2025-11-30'], None)
            s.commit_model(ok, None)
            if ok: s.last_invoice = True
        elif r < 0.75:
            j = json.loads(run(s.books, ['--json', 'bills']).stdout)['bills']
            inv = json.loads(run(s.books, ['--json', 'invoices']).stdout)
            debts = [str(b.get('id', b.get('entry'))) for b in j if b.get('open', {}).get('quantity', '0') not in ('0.00', '0')]
            debts += [i['number'] for i in inv if i['status'] == 'open']
            if debts:
                ok, _ = s.do(['pay', s.r.choice(debts), '--date', '2025-11-30'] + (['--amount', '1.00'] if s.r.random() < 0.5 else []), None)
                s.commit_model(ok, None)
        elif r < 0.85:
            e = json.loads(run(s.books, ['--json', 'entries']).stdout)
            cand = [x['entry'] for x in e if not x['reversed'] and not x['description'].startswith(('Invoice', 'Credit'))]
            if cand:
                ok, _ = s.do(['revert', str(s.r.choice(cand)), '--date', '2025-11-30'], None)
                s.commit_model(ok, None)
        else:
            ok, _ = s.do(['config', 'set', 'invoice.terms_days', str(s.r.randint(1, 60))], None)
            s.commit_model(ok, None)

    def verify(s, i):
        p = run(s.books, ['check'])
        if p.returncode != 0 or p.stdout.strip() not in ('', ) and 'no problem' not in p.stdout.lower():
            s.note(f'step {i}: check not clean: rc={p.returncode} {p.stdout[:300]} {p.stderr[:300]}')
        f, d, keeps = actual(s.books)
        if f != s.m.files or d != s.m.dirs:
            s.note(f'step {i}: tree differs after {s.trace[-1][:2]}\n  only abo files {set(f)-set(s.m.files)} dirs {d-s.m.dirs}\n  only model files {set(s.m.files)-set(f)} dirs {s.m.dirs-d}')
            s.m.files, s.m.dirs = f, d
        for dd in d:
            if dd not in keeps: s.note(f'step {i}: {dd} lacks .keep')
        for p_ in list(f) + list(d):
            if nfc(p_) != p_: s.note(f'step {i}: not NFC in git: {p_!r}')
        st = git(s.books, 'status', '--porcelain')
        if st.strip(): s.note(f'step {i}: worktree dirty: {st}')
        j = json.loads(run(s.books, ['--json', 'corporate', 'list']).stdout)
        if set(x['path'] for x in j['files']) != set(f) or set(j['directories']) != d:
            s.note(f'step {i}: corporate list differs from git')

    def go(s):
        for i in range(s.n):
            s.step()
            if i % 5 == 0 or i == s.n - 1: s.verify(i)
        return s.replay()

    def replay(s):
        lines = json.loads(run(s.books, ['--json', 'log', '--last', '100000']).stdout)
        again_parent = os.path.join(s.root, 'again'); again = os.path.join(again_parent, 'books')
        os.makedirs(again)
        env = dict(ENV, ABO_TODAY='2027-01-15')
        for n, l in enumerate(lines):
            argv = list(l['argv'])
            if argv[0] == 'file':
                b = subprocess.run(['git', 'show', f"{l['commit']}:{argv[-1]}"], cwd=s.books, capture_output=True).stdout
                doc = os.path.join(again_parent, argv[-1].replace('/', '-')); open(doc, 'wb').write(b); argv[-1] = doc
            if argv[:2] == ['corporate', 'add']:
                assert argv[2] == '--', argv
                for i in range(3, len(argv) - 1):
                    pth = argv[i]
                    b = subprocess.run(['git', 'show', f"{l['commit']}:{pth}"], cwd=s.books, capture_output=True).stdout
                    d = os.path.join(again_parent, f"c-{n}-{i}"); os.makedirs(d)
                    doc = os.path.join(d, pth.rsplit('/', 1)[1]); open(doc, 'wb').write(b); argv[i] = doc
            p = run(again, argv, env=env)
            if p.returncode != 0:
                s.note(f'replay line {n} {argv!r} failed: {p.stderr[:400]}'); return False
        # compare trees, invoice PDFs aside
        def tree(b):
            out = {}
            for p in git(b, 'ls-files', '-z').split('\0'):
                if not p or (p.startswith('invoices/') and p.endswith('.pdf')): continue
                data = open(os.path.join(b, p), 'rb').read()
                if p.startswith('invoices/') and p.endswith('.json'):
                    j = json.loads(data); j.pop('pdf', None); data = json.dumps(j, sort_keys=True).encode()
                out[p] = hashlib.sha1(data).hexdigest()
            return out
        a, b = tree(s.books), tree(again)
        if a != b:
            diff = {k: (a.get(k), b.get(k)) for k in set(a) | set(b) if a.get(k) != b.get(k)}
            s.note(f'replay differs: {diff}')
            return False
        return True

if __name__ == '__main__':
    seed = int(sys.argv[3]); n = int(sys.argv[4])
    f = Fuzz(seed, n); ok = f.go()
    json.dump(f.trace, open(os.path.join(f.root, 'trace.json'), 'w'), ensure_ascii=False, indent=0)
    print(f'seed {seed}: {len(f.issues)} issues, replay {"ok" if ok else "FAILED"}')
    sys.exit(0 if ok and not f.issues else 1)
