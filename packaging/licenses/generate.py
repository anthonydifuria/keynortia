#!/usr/bin/env python3
"""Writes THIRD-PARTY-LICENSES.md: the libraries and fonts Keynortia is made with, where each one lives
and the text of its license.

    python3 packaging/licenses/generate.py            rewrites THIRD-PARTY-LICENSES.md
    python3 packaging/licenses/generate.py --check    only checks that the file is up to date (exit 1 if not)

How it works:
  * `cargo tree` says which libraries are compiled for the systems the releases are built for;
  * every library is read from cargo's own copy (the published package), where its license files are;
  * what a package lacks is filled in from extra.toml (texts copied from the source repositories, kept
    unchanged in upstream/);
  * the fonts of the program come from assets/fonts.
Nothing is written from memory: a library with no license text at all stops the script with its name.

Run it again, and commit the result, whenever Cargo.lock or the fonts change. It needs cargo, Python 3.11 or
later, and the network the first time (cargo downloads the libraries it does not have yet).
"""

import argparse
import collections
import json
import os
import re
import subprocess
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # Python older than 3.11
    sys.exit("This script needs Python 3.11 or later (for tomllib).")

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
OUTPUT = ROOT / "THIRD-PARTY-LICENSES.md"

# The systems the releases are built for (see .github/workflows/release.yml).
TARGETS = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
]

# The fonts that live in assets/fonts: (name, what it is used for, web page, license file).
FONTS = [
    ("IBM Plex Mono", "the text of the interface", "https://github.com/IBM/plex", "OFL-IBMPlexMono.txt"),
    ("Cinzel", "the title", "https://github.com/NDISCOVER/Cinzel", "OFL-Cinzel.txt"),
    (
        "Noto Sans Old Italic",
        "the Etruscan letters",
        "https://github.com/notofonts/old-italic",
        "OFL-NotoSansOldItalic.txt",
    ),
]

# File names that carry a license or a copyright notice. They are looked for in the top folder of a package and
# two levels below it.
LICENSE_NAME = re.compile(r"^(licen[cs]e|copying|copyright|unlicense|notice|patents)([-_.][\w.+-]*)?$", re.I)
NOT_TEXT = {".rs", ".toml", ".json", ".py", ".c", ".h", ".js", ".yml", ".yaml", ".sh", ".xml", ".html"}
SKIP_DIRS = {"target", ".git", "tests", "test", "examples", "benches", "node_modules"}
LICENSE_DIRS = {"license", "licenses", "licence", "licences"}

COPYLEFT = re.compile(r"GNU\s+(Lesser\s+|Library\s+)?General\s+Public\s+License", re.I)

# When a library offers a choice of licenses ("MIT OR Apache-2.0") Keynortia uses the first of these it is offered.
PREFERENCE = [
    "MIT", "Zlib", "ISC", "BSD-2-Clause", "BSD-3-Clause", "0BSD", "Unlicense", "CC0-1.0", "Unicode-3.0",
    "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "OFL-1.1", "Ubuntu-font-1.0",
]

# The license each kind of text stands for, to leave out the texts of the licenses that were not chosen.
FAMILY_ID = {
    "MIT License": "MIT",
    "Apache License 2.0": "Apache-2.0",
    "Apache License 2.0 with LLVM exception": "Apache-2.0 WITH LLVM-exception",
    "zlib License": "Zlib",
    "ISC License": "ISC",
    "BSD 3-Clause License": "BSD-3-Clause",
    "BSD 2-Clause License": "BSD-2-Clause",
    "BSD Zero Clause License": "0BSD",
    "The Unlicense": "Unlicense",
    "Unicode License": "Unicode-3.0",
    "CC0 1.0 Universal": "CC0-1.0",
    "SIL Open Font License 1.1": "OFL-1.1",
    "Ubuntu Font Licence 1.0": "Ubuntu-font-1.0",
    "GNU General Public License 2": "GPL-2.0-only",
}

# The order the families of license texts come in, in the output.
FAMILY_ORDER = list(FAMILY_ID)

MIT_TEXT = """\
MIT License

Copyright (c) {holder}

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE."""


# ---------------------------------------------------------------------------------------------------------
# What goes into the program


def cargo(*args):
    result = subprocess.run(["cargo", *args], cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"cargo {' '.join(args)} failed:\n{result.stderr}")
    return result.stdout


def find_libraries():
    """The libraries compiled for at least one system: {(name, version): package info from cargo metadata}."""
    wanted, packages, root = set(), {}, None
    line_re = re.compile(r"^(\S+) v(\S+)")
    for target in TARGETS:
        tree = cargo("tree", "--locked", "--target", target, "-e", "normal,build", "--prefix", "none",
                     "--format", "{p}")
        first = True
        for line in tree.splitlines():
            if not line.strip() or line.startswith("["):
                continue
            match = line_re.match(line)
            if not match:
                sys.exit(f"Cannot read this line of `cargo tree`: {line!r}")
            if first:  # the first line is Keynortia itself
                root, first = (match[1], match[2]), False
            else:
                wanted.add((match[1], match[2]))
        metadata = json.loads(cargo("metadata", "--locked", "--format-version", "1", "--filter-platform", target))
        for package in metadata["packages"]:
            packages[(package["name"], package["version"])] = package
    wanted.discard(root)
    missing = [key for key in wanted if key not in packages]
    if missing:
        sys.exit(f"cargo metadata does not know these libraries: {missing}")
    return {key: packages[key] for key in sorted(wanted, key=lambda k: (k[0].lower(), k[1]))}


# ---------------------------------------------------------------------------------------------------------
# Which license is used: "MIT OR Apache-2.0" and the like


def parse_license(expression):
    """Reads an SPDX license expression into nested tuples: ("or", [...]), ("and", [...]) or ("id", "MIT")."""
    tokens = re.findall(r"\(|\)|[^\s()]+", expression.replace("/", " OR "))
    position = 0

    def peek():
        return tokens[position] if position < len(tokens) else None

    def take():
        nonlocal position
        position += 1
        return tokens[position - 1]

    def parse_or():
        parts = [parse_and()]
        while peek() == "OR":
            take()
            parts.append(parse_and())
        return parts[0] if len(parts) == 1 else ("or", parts)

    def parse_and():
        parts = [parse_atom()]
        while peek() == "AND":
            take()
            parts.append(parse_atom())
        return parts[0] if len(parts) == 1 else ("and", parts)

    def parse_atom():
        token = take()
        if token == "(":
            inner = parse_or()
            if take() != ")":
                raise ValueError("missing )")
            return inner
        if peek() == "WITH":
            take()
            return ("id", f"{token} WITH {take()}")
        return ("id", token)

    tree = parse_or()
    if position != len(tokens):
        raise ValueError("unexpected text")
    return tree


def choose_license(expression):
    """(licenses used, licenses offered) for a license expression. Of an "A OR B", the first in PREFERENCE is used."""

    def rank(license_id):
        return PREFERENCE.index(license_id) if license_id in PREFERENCE else len(PREFERENCE)

    def walk(node):
        kind, value = node
        if kind == "id":
            return {value}, {value}
        results = [walk(child) for child in value]
        offered = set().union(*(offered for _, offered in results))
        if kind == "and":
            return set().union(*(used for used, _ in results)), offered
        best = min(results, key=lambda r: (max(rank(i) for i in r[0]), len(r[0])))
        return best[0], offered

    try:
        return walk(parse_license(expression))
    except (ValueError, IndexError):
        return set(), set()  # an expression we cannot read: nothing is left out


# ---------------------------------------------------------------------------------------------------------
# Texts


def read_text(path):
    data = Path(path).read_bytes()
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        text = data.decode("latin-1")
    text = text.replace("\r\n", "\n").replace("\r", "\n").lstrip("﻿")
    return "\n".join(line.rstrip() for line in text.split("\n")).strip("\n")


def package_license_files(package_dir):
    """Paths (relative to the package) of the files that look like a license or a copyright notice."""
    found = []
    for dirpath, dirnames, filenames in os.walk(package_dir):
        relative = Path(dirpath).relative_to(package_dir)
        depth = len(relative.parts)
        dirnames[:] = sorted(d for d in dirnames if d.lower() not in SKIP_DIRS) if depth < 2 else []
        in_license_dir = depth > 0 and relative.parts[-1].lower() in LICENSE_DIRS
        for name in sorted(filenames):
            if in_license_dir or (LICENSE_NAME.match(name) and Path(name).suffix.lower() not in NOT_TEXT):
                found.append(relative / name)
    return sorted(found)


def family_of(name, text):
    """A name for the kind of text, used for the headings: from the file name for notices, else from the text."""
    stem = name.lower()
    if stem.startswith("copyright"):
        return "Copyright notice (COPYRIGHT file)"
    if stem.startswith("notice"):
        return "NOTICE file"
    if stem.startswith("copying") and not COPYLEFT.search(text):
        return "Licensing notice (COPYING file)"
    if stem.startswith("license.md") and text.lstrip().startswith("# License"):
        return "Licensing statement of a project"
    upper = " ".join(text.upper().split())
    if "SIL OPEN FONT LICENSE" in upper:
        return "SIL Open Font License 1.1"
    if "UBUNTU FONT LICENCE" in upper:
        return "Ubuntu Font Licence 1.0"
    if "BITSTREAM VERA LICENSE" in upper:
        return "MIT and Bitstream Vera licenses"
    if COPYLEFT.search(text):
        if "GNU LESSER GENERAL PUBLIC LICENSE VERSION 2.1" in upper:
            return "GNU Lesser General Public License 2.1"
        if "GNU LIBRARY GENERAL PUBLIC LICENSE VERSION 2" in upper:
            return "GNU Library General Public License 2"
        if "GNU GENERAL PUBLIC LICENSE VERSION 2" in upper:
            return "GNU General Public License 2"
        return "GNU license"
    if "APACHE LICENSE" in upper and "VERSION 2.0" in upper:
        return "Apache License 2.0 with LLVM exception" if "LLVM EXCEPTION" in upper else "Apache License 2.0"
    if "UNICODE LICENSE V3" in upper or "UNICODE, INC. LICENSE" in upper:
        return "Unicode License"
    if "CREATIVE COMMONS LEGAL CODE" in upper and "CC0" in upper:
        return "CC0 1.0 Universal"
    if "BY OBTAINING, USING, AND/OR COPYING THIS SOFTWARE" in upper:
        return "MIT-CMU License"
    if "THIS IS FREE AND UNENCUMBERED SOFTWARE" in upper:
        return "The Unlicense"
    if "PERMISSION IS HEREBY GRANTED, FREE OF CHARGE" in upper:
        return "MIT License"
    if "THIS SOFTWARE IS PROVIDED 'AS-IS'" in upper or "THIS SOFTWARE IS PROVIDED 'AS IS'" in upper:
        return "zlib License"
    if "REDISTRIBUTION AND USE IN SOURCE AND BINARY FORMS" in upper:
        return "BSD 3-Clause License" if "NEITHER THE NAME" in upper else "BSD 2-Clause License"
    if "PERMISSION TO USE, COPY, MODIFY, AND/OR DISTRIBUTE THIS SOFTWARE" in upper:
        return "ISC License" if "ABOVE COPYRIGHT NOTICE AND THIS PERMISSION NOTICE" in upper else "BSD Zero Clause License"
    first = next((line.strip() for line in text.splitlines() if line.strip()), name)
    return "Other: " + first[:70]


def holder_line(text):
    """The first real copyright line of a text, without e-mail addresses, to tell texts of one family apart."""
    for line in text.splitlines():
        stripped = line.strip()
        if not re.match(r"^\W*(Copyright\b|©|\(c\)\s*\d)", stripped) or len(stripped) < 12:
            continue
        if re.search(r"\[yyyy\]|\{yyyy\}|<year>|\[name of|\{name of|copyright owner|copyright holder", stripped, re.I):
            continue  # the empty form of a license, not a notice
        return re.sub(r"\s*<[^>]*@[^>]*>", "", stripped)[:110]
    return ""


def fenced(text):
    longest = max((len(run) for run in re.findall(r"`+", text)), default=0)
    fence = "`" * max(3, longest + 1)
    return f"{fence}text\n{text}\n{fence}\n"


def crate_matches(selector, key):
    name, _, version = selector.partition("@")
    return key[0] == name and (not version or key[1] == version)


def sentence(text):
    return text.rstrip(".") + "."


# ---------------------------------------------------------------------------------------------------------
# The file


def web_link(url):
    """A repository address as the text of a link: without the scheme and the trailing slash."""
    return re.sub(r"^https?://", "", url).rstrip("/")


def clean_repository(url):
    url = url.strip().rstrip("/")
    if url.endswith(".git"):
        url = url[:-4]
    return re.sub(r"^http://(github\.com|gitlab\.com|codeberg\.org)", r"https://\1", url)


def build():
    extra = tomllib.loads((HERE / "extra.toml").read_text(encoding="utf-8"))
    libraries = find_libraries()
    problems, warnings = [], []
    used_licenses = {}

    # text without its line breaks -> {"text", "family", "holder", "users": {library: {where it came from}}}
    blocks = {}

    def add(key, name, text, origin=""):
        text = text.strip("\n")
        # texts that differ only in line breaks or indentation are the same text: one copy is shown
        block = blocks.setdefault(
            " ".join(text.split()),
            {"text": text, "family": family_of(name, text), "holder": holder_line(text), "users": {}},
        )
        block["users"].setdefault(key, set())
        if origin:
            block["users"][key].add(origin)

    used_entries = collections.Counter()

    for key, package in libraries.items():
        package_dir = Path(package["manifest_path"]).parent
        expression = package.get("license") or ""
        chosen, offered = choose_license(expression)
        used_licenses[key] = chosen
        not_used = offered - chosen

        # every text found for this library: (file name, text, origin, belongs to the library as a whole)
        found = []
        for relative in package_license_files(package_dir):
            found.append((relative.name, read_text(package_dir / relative), "", len(relative.parts) == 1))
        for entry in extra.get("package_files", []):
            if crate_matches(entry["crate"], key):
                used_entries["package_files:" + entry["crate"]] += 1
                for name in entry["files"]:
                    found.append((Path(name).name, read_text(package_dir / name), "", False))
        for source in extra.get("source", []):
            if any(crate_matches(s, key) for s in source["crates"]):
                used_entries["source:" + source["url"]] += 1
                origin = f"Not in the published package: copied unchanged from {source['url']}"
                for name in source["files"]:
                    found.append((Path(name).name, read_text(HERE / name), origin, True))
        for standard in extra.get("standard", []):
            if any(crate_matches(s, key) for s in standard["crates"]):
                used_entries["standard:" + standard["holder"]] += 1
                if standard["license"] != "MIT":
                    problems.append(f"extra.toml: generate.py has no standard text for {standard['license']}")
                    continue
                origin = f"Standard text, not copied from the library: {standard['reason'].rstrip('.')}"
                found.append(("LICENSE", MIT_TEXT.format(holder=standard["holder"]), origin, True))

        # leave out the texts of the licenses that the library offers and Keynortia does not use
        kept = [f for f in found if not (f[3] and FAMILY_ID.get(family_of(f[0], f[1])) in not_used)]
        if not kept:
            kept = found
        if not kept:
            problems.append(f"{key[0]} {key[1]} ({expression}): no license text found; add it to extra.toml")
        for name, text, origin, _ in kept:
            if COPYLEFT.search(text) and not any(crate_matches(a["crate"], key) for a in extra.get("allow_copyleft", [])):
                problems.append(f"{key[0]} {key[1]}: {name} is a copyleft license text; add the library to "
                                "[[allow_copyleft]] in extra.toml if that is meant")
                continue
            add(key, name, text, origin)

    # entries of extra.toml that no longer match anything are probably out of date
    for section, field in (("source", "url"), ("standard", "holder")):
        for entry in extra.get(section, []):
            if not used_entries[f"{section}:{entry[field]}"]:
                warnings.append(f"extra.toml: [[{section}]] {entry['crates']} matches none of the libraries")
    for entry in extra.get("package_files", []):
        if not used_entries["package_files:" + entry["crate"]]:
            warnings.append(f"extra.toml: [[package_files]] {entry['crate']} matches none of the libraries")

    if problems:
        sys.exit("Cannot write the file:\n  " + "\n  ".join(problems))
    for warning in warnings:
        print("warning:", warning, file=sys.stderr)

    return render(extra, libraries, used_licenses, blocks)


def render(extra, libraries, used_licenses, blocks):
    out = []
    emit = out.append

    emit("# Third-party licenses\n")
    emit(
        "Keynortia is free software under the [MIT License](LICENSE). It is made with the fonts and the open-source "
        "libraries listed here, and their licenses ask for their notices to go with the program: this file is where "
        "they are. Thank you to everyone who works on them.\n"
    )
    emit("*This file is written by `packaging/licenses/generate.py` from `Cargo.lock`: please do not edit it by "
         "hand. Run `python3 packaging/licenses/generate.py` to write it again.*\n")
    emit("- [Fonts](#fonts)")
    emit(f"- [Libraries](#libraries) ({len(libraries)})")
    emit("- [License texts](#license-texts)\n")

    # ---- fonts
    emit("## Fonts\n")
    emit(
        "These fonts are inside the program, and the web page uses the same ones. They are used and shared "
        "unchanged, under the SIL Open Font License 1.1. The copyright notice and the license of each one are "
        "below, and also in [`assets/fonts`](assets/fonts).\n"
    )
    for name, use, link, filename in FONTS:
        emit(f"### {name}\n")
        emit(f"Used for {use}. [{web_link(link)}]({link})\n")
        emit(fenced(read_text(ROOT / "assets" / "fonts" / filename)))

    # ---- libraries
    emit("## Libraries\n")
    emit(
        f"{len(libraries)} libraries (Rust crates from [crates.io](https://crates.io)) are compiled into the "
        "program for Linux (x86_64, arm64), Windows (x86_64) and macOS (Apple silicon, Intel). Each system uses "
        "only some of them, and a few are used only while building. Like every Rust program, Keynortia also "
        "contains the Rust standard library, under the MIT license and the Apache License 2.0 "
        "([github.com/rust-lang/rust](https://github.com/rust-lang/rust)).\n"
    )
    emit(
        "*License* is what the library says in its `Cargo.toml`. When it offers a choice (\"MIT OR Apache-2.0\"), "
        "Keynortia uses it under one of them, MIT whenever it is offered, and *Used under* says which.\n"
    )
    emit("| Library | Version | License | Used under | Source |")
    emit("|---|---|---|---|---|")
    for key, package in libraries.items():
        name, version = key
        declared = (package.get("license") or f"see {package.get('license_file') or 'its license file'}").replace("|", "\\|")
        used = ", ".join(sorted(used_licenses[key], key=lambda i: PREFERENCE.index(i) if i in PREFERENCE else 99)) or "-"
        repository = package.get("repository") or package.get("homepage") or ""
        source = f"[{web_link(clean_repository(repository))}]({clean_repository(repository)})" if repository else ""
        emit(f"| [{name}](https://crates.io/crates/{name}/{version}) | {version} | {declared} | {used} | {source} |")
    emit("")
    notes = [n for n in extra.get("note", []) if any(crate_matches(n["crate"], key) for key in libraries)]
    if notes:
        emit("Notes:\n")
        for note in notes:
            emit(f"- **{note['crate']}**: {note['text']}")
        emit("")

    # ---- texts
    emit("## License texts\n")
    emit(
        "Each text is copied from the library, or from its source repository when the published package has none "
        "(a line says so under the list of libraries). Texts that differ only in line breaks or indentation are "
        "shown once. Only the texts of the licenses that Keynortia uses are given.\n"
    )
    by_family = collections.defaultdict(list)
    for block in blocks.values():
        by_family[block["family"]].append(block)

    def family_key(family):
        return (FAMILY_ORDER.index(family), family) if family in FAMILY_ORDER else (len(FAMILY_ORDER), family)

    for family in sorted(by_family, key=family_key):
        entries = sorted(by_family[family], key=lambda b: (b["holder"].lower(), min(b["users"])))
        titles = [family + (f" · {b['holder']}" if b["holder"] else "") for b in entries]
        emit(f"### {family}\n")
        seen = collections.Counter()
        for block, title in zip(entries, titles):
            if titles.count(title) > 1:  # several texts with the same heading: number them
                seen[title] += 1
                title += f" ({seen[title]})"
            emit(f"#### {title}\n")
            users = sorted(block["users"], key=lambda k: (k[0].lower(), k[1]))
            names = ", ".join(f"[{n} {v}](https://crates.io/crates/{n}/{v})" for n, v in users)
            emit(f"Used by: {names}\n")
            origins = collections.defaultdict(list)
            for key in users:
                for origin in sorted(block["users"][key]):
                    origins[origin].append(f"{key[0]} {key[1]}")
            for origin, who in origins.items():
                if len(origins) == 1 and len(who) == len(users):
                    emit(f"*{sentence(origin)}*\n")
                else:
                    emit(f"*For {', '.join(who)}: {sentence(origin[0].lower() + origin[1:])}*\n")
            emit(fenced(block["text"]))

    return "\n".join(out).rstrip("\n") + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--check", action="store_true", help="do not write: exit 1 if the file is out of date")
    args = parser.parse_args()
    content = build()
    if args.check:
        current = OUTPUT.read_text(encoding="utf-8") if OUTPUT.exists() else ""
        if current != content:
            sys.exit(f"{OUTPUT.name} is out of date: run python3 packaging/licenses/generate.py")
        print(f"{OUTPUT.name} is up to date.")
        return
    OUTPUT.write_text(content, encoding="utf-8")
    print(f"Wrote {OUTPUT.name}: {len(content.encode()) // 1024} KB")


if __name__ == "__main__":
    main()
