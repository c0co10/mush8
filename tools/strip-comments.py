#!/usr/bin/env python3
# Mush8 - strip-comments.py
# Constantin Clerc (c22dev) - 0.1
import os, re, subprocess, sys

HEADER = "// Mush8 - {name}\n// Constantin Clerc (c22dev) - 0.1\n"
SH_HEADER = "# Mush8 - {name}\n# Constantin Clerc (c22dev) - 0.1\n"


def strip_c_like(src):
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            while i < n and src[i] != "\n":
                i += 1
        elif src.startswith("/*", i):
            start = i
            i += 2
            while i < n and not src.startswith("*/", i):
                i += 1
            i += 2
            out.append("\n" * src[start:i].count("\n") or " ")
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            out.append(src[i:j + 1])
            i = j + 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


def is_msl(body):
    return "#include <metal_stdlib>" in body or "using namespace metal" in body or "[[" in body and "kernel" in body or "vertex " in body and "fragment " in body


def strip_rust(src):
    out = []
    i, n = 0, len(src)

    def ident_char(ch):
        return ch.isalnum() or ch == "_"

    while i < n:
        c = src[i]
        if src.startswith("//", i):
            while i < n and src[i] != "\n":
                i += 1
            continue
        if src.startswith("/*", i):
            start = i
            depth, i = 1, i + 2
            while i < n and depth:
                if src.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif src.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            out.append("\n" * src[start:i].count("\n") or " ")
            continue
        m = re.compile(r'(b|c)?r(#*)"').match(src, i)
        if m and (i == 0 or not ident_char(src[i - 1])):
            close = '"' + m.group(2)
            j = src.find(close, m.end())
            assert j >= 0
            body = src[m.end():j]
            if is_msl(body):
                body = clean_lines(body, strip_c_like(body), keep_edges=True)
            out.append(src[i:m.end()] + body + close)
            i = j + len(close)
            continue
        if c == '"' or (c in "bc" and src.startswith('"', i + 1) and (i == 0 or not ident_char(src[i - 1]))):
            j = i + (1 if c == '"' else 2)
            while src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            out.append(src[i:j + 1])
            i = j + 1
            continue
        if c == "'" or (c == "b" and src.startswith("'", i + 1) and (i == 0 or not ident_char(src[i - 1]))):
            q = i if c == "'" else i + 1
            m = re.compile(r"'(\\(x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^\\'\n])'", re.S).match(src, q)
            if m:
                out.append(src[i:m.end()])
                i = m.end()
                continue
            out.append(src[i:q + 1])
            i = q + 1
            continue
        if ident_char(c):
            j = i
            while j < n and ident_char(src[j]):
                j += 1
            out.append(src[i:j])
            i = j
            continue
        out.append(c)
        i += 1
    return "".join(out)


def strip_swift(src):
    out = []
    i, n = 0, len(src)

    def skip_string(i):
        m = re.compile(r'(#*)("""|")').match(src, i)
        hashes, q = m.group(1), m.group(2)
        close = q + hashes
        esc = "\\" + hashes
        j = m.end()
        while not src.startswith(close, j):
            if src.startswith(esc + "(", j):
                j = skip_code(j + len(esc) + 1, ")")
            elif src.startswith(esc, j):
                j += len(esc) + 1
            else:
                j += 1
        return j + len(close)

    def skip_code(j, end):
        depth = 0
        while True:
            ch = src[j]
            if ch == '"' or (ch == "#" and re.compile(r'#+"').match(src, j)):
                j = skip_string(j)
            elif ch == "(":
                depth += 1
                j += 1
            elif ch == ")":
                if depth == 0:
                    return j + 1
                depth -= 1
                j += 1
            else:
                j += 1

    while i < n:
        c = src[i]
        if src.startswith("//", i):
            while i < n and src[i] != "\n":
                i += 1
        elif src.startswith("/*", i):
            start = i
            depth, i = 1, i + 2
            while depth:
                if src.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif src.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            out.append("\n" * src[start:i].count("\n") or " ")
        elif c == '"' or (c == "#" and re.compile(r'#+"').match(src, i)):
            j = skip_string(i)
            out.append(src[i:j])
            i = j
        else:
            out.append(c)
            i += 1
    return "".join(out)


def strip_sh(src):
    lines = src.split("\n")
    out = []
    for k, line in enumerate(lines):
        if k == 0 and line.startswith("#!"):
            out.append(line)
        elif line.lstrip().startswith("#"):
            out.append("\0")
        else:
            out.append(line)
    return "\n".join(out)


def clean_lines(orig, stripped, keep_edges=False):
    o_lines = orig.split("\n")
    s_lines = stripped.split("\n")
    res = []
    if len(o_lines) == len(s_lines):
        for o, s in zip(o_lines, s_lines):
            if s.strip() == "" and o.strip() != "":
                continue
            res.append(s.rstrip() if s != o else s)
    else:
        for s in s_lines:
            res.append(s.rstrip())
    text = "\n".join(res)
    text = re.sub(r"\n{3,}", "\n\n", text)
    if not keep_edges:
        text = text.lstrip("\n")
    return text


def process(path):
    name = os.path.basename(path)
    src = open(path).read()
    if path.endswith(".sh"):
        s = strip_sh(src)
        body = "\n".join(l for l in s.split("\n") if l != "\0")
        lines = body.split("\n")
        shebang = lines[0] + "\n" if lines[0].startswith("#!") else ""
        rest = "\n".join(lines[1:] if shebang else lines).lstrip("\n")
        new = shebang + SH_HEADER.format(name=name) + rest
    else:
        s = strip_rust(src) if path.endswith((".rs", ".c")) else strip_swift(src)
        new = HEADER.format(name=name) + clean_lines(src, s)
    new = new.rstrip("\n") + "\n"
    if new != src:
        open(path, "w").write(new)

EXTS = (".rs", ".swift", ".sh", ".c")


VENDORED = ("crates/vpx/libvpx/",)
SKIP_DIRS = {"work", "target", "Firmware", "system", "build", ".git"}


def project_root():
    try:
        found = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip()
        if found:
            return found
    except (subprocess.CalledProcessError, FileNotFoundError):
        pass
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def walk_files(root):
    listed = []
    for dirpath, dirnames, filenames in os.walk(root):
        rel = os.path.relpath(dirpath, root)
        if rel == ".":
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS and not d.endswith(".xcodeproj")]
        else:
            dirnames[:] = [d for d in dirnames if not d.endswith(".xcodeproj")]
        for name in filenames:
            listed.append(name if rel == "." else os.path.join(rel, name))
    return listed


def project_files():
    root = project_root()
    os.chdir(root)
    try:
        listed = subprocess.run(["git", "ls-files", "--cached", "--others", "--exclude-standard"], capture_output=True, text=True, check=True).stdout.split("\n")
    except (subprocess.CalledProcessError, FileNotFoundError):
        listed = walk_files(root)
    return sorted({p for p in listed if p.endswith(EXTS) and ".xcodeproj/" not in p and not p.startswith(VENDORED) and os.path.isfile(p)})


for p in sys.argv[1:] or project_files():
    before = open(p).read()
    process(p)
    if open(p).read() != before:
        print("cleaned:", p)
