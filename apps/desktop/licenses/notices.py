#!/usr/bin/env python3
"""Regenerate THIRD_PARTY_NOTICES.txt and the CycloneDX SBOM for Nexus Desktop (GPUI).

Inputs: the list in licenses/inputs.toml (Cargo manifests and targets, the npm bundles' inventories and legal files,
CEF's license, the fonts and marks). Paths there are relative to the GPUI workspace, this script's parent folder.

  licenses/notices.py               rewrite the committed THIRD_PARTY_NOTICES.txt
  licenses/notices.py --check       exit 1 when the committed THIRD_PARTY_NOTICES.txt is stale
  licenses/notices.py --out DIR     write THIRD_PARTY_NOTICES.txt and sbom.cdx.json to DIR, and Chromium's
                                    CREDITS.html too when $CEF_PATH holds the CEF distribution

Tools, at these exact versions: cargo-about 0.9.3 (the notices) and cargo-cyclonedx 0.5.9 (the SBOM, --out only).
Both read the local Cargo registry only (`--frozen`), so run `cargo fetch --locked` first on a fresh machine.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CARGO_ABOUT = "0.9.3"
CARGO_CYCLONEDX = "0.5.9"
RULE = "=" * 80
LINE = "-" * 80
LOCAL_URL = re.compile(r"\?download_url=file://[^#]*")
# License files that name the crate's own license choices. Any other LICENSE-<suffix> file is an extra notice
# (bundled third-party code, such as ring's BoringSSL or GPUI Kit's Lucide icons) and goes in verbatim.
STANDARD_SUFFIXES = {"", "mit", "apache", "apache2", "apache20", "zlib", "bsd", "bsd3clause", "isc", "unicode",
                     "boost", "cc0", "0bsd", "gplv2", "apache20withllvmexception", "spdx"}
NOTICE_NAME = re.compile(r"(?i)^(copyright|notice|third[-_]?party|thirdpartynotices)")
LICENSE_NAME = re.compile(r"(?i)^(licen[cs]e|copying)(.*)$")


def load_inputs():
    with open(ROOT / "licenses" / "inputs.toml", "rb") as f:
        return tomllib.load(f)


def text_of(path):
    """A text file with its line endings made Unix ones, so the output is the same on every machine."""
    return path.read_bytes().decode("utf-8", errors="replace").replace("\r\n", "\n").replace("\r", "\n")


def require(tool, version):
    try:
        out = subprocess.run(["cargo", tool, "--version"], capture_output=True, text=True, check=True).stdout
    except (OSError, subprocess.CalledProcessError):
        out = ""
    if version not in out.split():
        flags = " --features cli" if tool == "about" else ""
        sys.exit(f"needs cargo-{tool} {version} (found: {out.strip() or 'none'}); "
                 f"install it with `cargo install --locked cargo-{tool} --version {version}{flags}`")


def cef_version():
    """CEF's version: the build metadata of the `cef` crate in Cargo.lock (`156.3.0+156.0.15` is CEF 156.0.15)."""
    m = re.search(r'name = "cef"\nversion = "[^"+]+\+([^"]+)"', (ROOT / "Cargo.lock").read_text())
    return m.group(1) if m else "unknown"


def cef_full_version(inputs):
    """CEF's full version (`156.0.15+gcf671c0+chromium-156.0.8078.80`), from inputs.toml, checked against Cargo.lock."""
    full = inputs["cef"]["full_version"]
    if not full.startswith(cef_version() + "+"):
        sys.exit(f"licenses/inputs.toml: cef.full_version {full} is not CEF {cef_version()} from Cargo.lock; "
                 "copy CEF_VERSION from the cef-dll-sys bindings")
    return full


def extra_notices(package):
    """A crate's license files beyond its chosen license: every file of an `AND` expression, notice and copyright
    files, and LICENSE-<suffix> files that name no standard license."""
    folder = Path(package["manifest_path"]).parent
    every = " AND " in (package.get("license") or "")
    found = []
    for f in sorted(folder.iterdir()):
        if not f.is_file():
            continue
        m = LICENSE_NAME.match(f.name)
        suffix = re.sub(r"[^a-z0-9]", "", re.sub(r"(?i)\.(txt|md)$", "", m.group(2)).lower()) if m else None
        if NOTICE_NAME.match(f.name) or (m and (every or suffix not in STANDARD_SUFFIXES)):
            found.append((f.name, text_of(f)))
    return found


def cargo_about(inputs):
    """cargo-about's JSON for every manifest: the licenses, each with the crates that use it, and extra notices."""
    require("about", CARGO_ABOUT)
    licenses, extras = {}, {}
    for manifest in inputs["cargo_manifests"]:
        cmd = ["cargo", "about", "generate", "--frozen", "--fail", "--format", "json",
               "-c", str(ROOT / inputs["about_config"]), "-m", str(ROOT / manifest)]
        for target in inputs["targets"]:
            cmd += ["--target", target]
        data = json.loads(subprocess.run(cmd, capture_output=True, text=True, check=True, cwd=ROOT).stdout)
        for lic in data["licenses"]:
            text = lic["text"].replace("\r\n", "\n").replace("\r", "\n")
            entry = licenses.setdefault((lic["id"], text), {"id": lic["id"], "name": lic["name"], "crates": {}})
            for use in lic["used_by"]:
                c = use["crate"]
                entry["crates"][(c["name"], c["version"])] = c.get("repository") or ""
        for c in data["crates"]:
            p = c["package"]
            files = extra_notices(p)
            if files:
                extras[(p["name"], p["version"])] = files
    return licenses, extras


def npm_packages(inputs):
    bundles = []
    for bundle in inputs.get("npm_bundles", []):
        inventory = json.loads(text_of(ROOT / bundle["inventory"]))
        legal = [(p, text_of(ROOT / p)) for p in bundle.get("legal_files", [])]
        bundles.append((bundle["name"], inventory["packages"], legal))
    return bundles


def render(inputs, rust, bundles):
    licenses, extras = rust
    out = []
    w = out.append
    crates = {k for e in licenses.values() for k in e["crates"]}
    npm_count = sum(len(pkgs) for _, pkgs, _ in bundles)
    w("THIRD-PARTY SOFTWARE NOTICES AND INFORMATION")
    w("")
    w("Nexus Desktop includes the third-party software listed below, under the licenses that follow.")
    w(f"Rust crates: {len(crates)}. npm packages: {npm_count}. Also the Chromium Embedded Framework (CEF), Chromium,")
    w("and the fonts and marks in part 4.")
    w("This file is generated by licenses/notices.py from licenses/inputs.toml. Do not edit it by hand.")
    w("")

    w(RULE)
    w("PART 1. RUST CRATES")
    w(f"Targets: {', '.join(inputs['targets'])}.")
    w(RULE)
    for (_, text), e in sorted(licenses.items(), key=lambda kv: (kv[0][0], min(kv[1]["crates"]))):
        w("")
        w(f"{e['name']} ({e['id']})")
        w("Used by:")
        for (name, version), repo in sorted(e["crates"].items()):
            w(f"  {name} {version}" + (f" ({repo})" if repo else ""))
        w("")
        w(text.rstrip())
        w(LINE)
    w("")
    w("Further notices in the crates' sources (bundled third-party code, copyright files, all parts of an AND license):")
    for (name, version), files in sorted(extras.items()):
        for fname, text in files:
            w("")
            w(f"{name} {version}: {fname}")
            w("")
            w(text.rstrip())
            w(LINE)

    for name, packages, legal in bundles:
        w("")
        w(RULE)
        w(f"PART 2. NPM PACKAGES BUNDLED IN {name}")
        w(RULE)
        for p in packages:
            w("")
            w(f"{p['name']} {p['version']}" + (f" ({p['license']})" if p.get("license") else ""))
            if p.get("repository"):
                w(f"  Source: {p['repository']}")
            for lt in p["license_texts"]:
                w("")
                w(lt["text"].replace("\r\n", "\n").rstrip())
            w(LINE)
        for path, text in legal:
            w("")
            w(f"{Path(path).name}:")
            w("")
            w(text.rstrip())
            w(LINE)

    cef = inputs["cef"]
    w("")
    w(RULE)
    w(f"PART 3. CHROMIUM EMBEDDED FRAMEWORK {cef_version()} AND CHROMIUM")
    w(RULE)
    w("")
    w(text_of(ROOT / cef["license_file"]).rstrip())
    w("")
    w(f"Chromium's own notices and those of its third-party code are in {cef['credits_file']}, a file of the CEF")
    w(f"{cef_version()} distribution. It is too large to repeat here. Every release package carries it next to this")
    w("file (CHROMIUM-CREDITS.html in the macOS app's Resources folder), and the packaging scripts fail without it.")
    w("")
    w("Written offer for the source of libcef (LGPL)")
    w("")
    w("libcef (libcef.so, libcef.dll and the Chromium Embedded Framework on macOS) is built from Chromium and includes FFmpeg,")
    w("which is licensed under the GNU Lesser General Public License (LGPL) version 2.1 or later. The full text of the LGPL")
    w(f"2.1 is in FFmpeg's entry in {cef['credits_file']}. libcef loads as a separate shared library, so you can rebuild it")
    w("from source and replace it.")
    w("")
    full = cef_full_version(inputs)
    chromium = full.rsplit("+chromium-", 1)[-1]
    w(f"This package ships the CEF project's own binary build {full}, unmodified, as published on")
    w(f"https://cef-builds.spotifycdn.com. It is built from CEF at that commit and from Chromium {chromium}, with the")
    w("default build settings, which leave the proprietary codecs (H.264, AAC) out.")
    w("")
    w("For three years from the date you received this package, Nexus Laboratories, Inc. will give anyone who asks a")
    w("complete machine-readable copy of the corresponding source code of that CEF and Chromium version, FFmpeg included,")
    w("with the scripts used to build it, on a physical medium or by download. We charge no more than our cost of doing so.")
    w("The offer is open to anyone, not only to people who received the package from us. To ask, write to")
    w("trademarks@nexus.inc, and say the package version and your platform.")
    w("")

    w(RULE)
    w("PART 4. FONTS AND MARKS")
    w(RULE)
    for asset in inputs.get("assets", []):
        w("")
        w(f"{asset['name']} ({asset['license']})")
        if asset.get("notice"):
            w("")
            w(asset["notice"].rstrip())
        if asset.get("license_file"):
            w("")
            w(text_of(ROOT / asset["license_file"]).rstrip())
        elif not asset.get("notice"):
            sys.exit(f"{asset['name']}: an asset needs a license_file or a notice in licenses/inputs.toml")
        w(LINE)

    ported = inputs.get("ported", [])
    if ported:
        w("")
        w(RULE)
        w("PART 5. CODE PORTED FROM OTHER PROJECTS")
        w(RULE)
    for p in ported:
        w("")
        w(f"{p['name']} ({p['license']})")
        w(f"  Source: {p['source']}")
        w(f"  In: {', '.join(p['paths'])}")
        w("")
        w(text_of(ROOT / p["license_file"]).rstrip())
        w(LINE)
    w("")
    return "\n".join(out)


def generate(inputs):
    return render(inputs, cargo_about(inputs), npm_packages(inputs))


def sbom(inputs, bundles, dest):
    """One CycloneDX 1.5 document: the Rust graph for every target, the npm bundles' packages and CEF."""
    require("cyclonedx", CARGO_CYCLONEDX)
    env = dict(os.environ, SOURCE_DATE_EPOCH=os.environ.get("SOURCE_DATE_EPOCH", "0"))
    doc, components, deps = None, {}, {}
    for manifest in inputs["cargo_manifests"]:
        produced = (ROOT / manifest).parent / "bom.json"
        for target in inputs["targets"]:
            subprocess.run(["cargo", "cyclonedx", "-q", "-f", "json", "--spec-version", "1.5", "--no-build-deps",
                            "--target", target, "--manifest-path", str(ROOT / manifest),
                            "--override-filename", "bom"], check=True, cwd=ROOT, env=env)
            try:
                part = json.loads(produced.read_text())
            finally:
                produced.unlink(missing_ok=True)
            if doc is None:
                doc = part
            for c in part.get("components", []):
                components.setdefault(c["bom-ref"], c)
            for d in part.get("dependencies", []):
                deps.setdefault(d["ref"], set()).update(d.get("dependsOn", []))
    if doc is None:
        sys.exit("inputs.toml lists no Cargo manifest")

    # Local paths (path dependencies, the vendored crates) become plain names: the SBOM must not carry a build folder.
    root = doc["metadata"]["component"]
    names: dict[str, str] = {ref: f"path:{c['name']}@{c['version']}"
                             for ref, c in [*components.items(), (root["bom-ref"], root)]
                             if ref.startswith("path+file://")}

    def clean(c: dict) -> None:
        ref, _, rest = str(c["bom-ref"]).partition(" ")
        c["bom-ref"] = names.get(ref, ref) + (f" {rest}" if rest else "")
        if "purl" in c:
            c["purl"] = LOCAL_URL.sub("", c["purl"])
        for sub in c.get("components", []):
            clean(sub)

    for c in components.values():
        clean(c)
    clean(root)

    extra = []
    for bundle_name, packages, _ in bundles:
        for p in packages:
            scoped = p["name"].replace("@", "%40", 1)
            c = {"type": "library", "bom-ref": f"npm:{p['name']}@{p['version']}", "name": p["name"],
                 "version": p["version"], "scope": "required", "purl": f"pkg:npm/{scoped}@{p['version']}",
                 "properties": [{"name": "bundled-in", "value": bundle_name}]}
            if p.get("license"):
                c["licenses"] = [{"expression": p["license"]}]
            extra.append(c)
    v = cef_version()
    extra.append({"type": "framework", "bom-ref": f"cef@{v}", "name": "Chromium Embedded Framework", "version": v,
                  "scope": "required", "purl": f"pkg:generic/cef@{v}",
                  "licenses": [{"license": {"id": "BSD-3-Clause"}}]})
    for c in extra:
        components[c["bom-ref"]] = c

    doc["components"] = sorted(components.values(), key=lambda c: c["bom-ref"])
    merged = {}
    for ref, on in deps.items():
        merged.setdefault(names.get(ref, ref), set()).update(names.get(r, r) for r in on)
    merged.setdefault(root["bom-ref"], set()).update(c["bom-ref"] for c in extra)
    doc["dependencies"] = [{"ref": r, "dependsOn": sorted(on)} for r, on in sorted(merged.items())]
    doc.pop("serialNumber", None)
    text = json.dumps(doc, indent=2) + "\n"
    if str(ROOT.parent) in text or "file://" in text:
        sys.exit("the SBOM still names a local folder; extend the path rewrite in sbom()")
    dest.write_text(text)
    return len(doc["components"])


def first_party_licenses(inputs):
    """Fail unless every Nexus crate the app builds declares the project's license. A Nexus crate is a path
    dependency outside vendor/ (vendor/ holds patched third-party crates under their own licenses). cargo-deny checks
    that a license is allowed; this checks that ours is the one the project ships under."""
    wanted = inputs["first_party_license"]
    wrong = set()
    for manifest in inputs["cargo_manifests"]:
        meta = json.loads(subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--frozen", "--manifest-path", str(ROOT / manifest)],
            capture_output=True, text=True, check=True, cwd=ROOT).stdout)
        vendor = Path(meta["workspace_root"]) / "vendor"
        for p in meta["packages"]:
            if p["source"] is None and vendor not in Path(p["manifest_path"]).parents and p.get("license") != wanted:
                wrong.add(f"{p['name']} ({p.get('license') or 'no license field'})")
    if wrong:
        sys.exit(f'these Nexus crates must declare license = "{wanted}": {", ".join(sorted(wrong))}')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="fail when the committed notices are stale")
    ap.add_argument("--out", type=Path, help="write the notices, the SBOM and CREDITS.html to this folder")
    args = ap.parse_args()
    inputs = load_inputs()
    first_party_licenses(inputs)
    committed = ROOT / inputs["outputs"]["notices"]

    if args.check:
        if not committed.exists() or committed.read_bytes() != generate(inputs).encode():
            sys.exit(f"{committed.name} is stale: run licenses/notices.py and commit the result")
        print(f"{committed.name} is up to date")
        return

    if args.out is None:
        committed.write_bytes(generate(inputs).encode())
        print(f"wrote {committed}")
        return

    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    (out / inputs["outputs"]["notices"]).write_bytes(generate(inputs).encode())
    count = sbom(inputs, npm_packages(inputs), out / inputs["outputs"]["sbom"])
    print(f"wrote {inputs['outputs']['notices']} and {inputs['outputs']['sbom']} ({count} components) to {out}")
    cef_path = os.environ.get("CEF_PATH")
    credits = inputs["cef"]["credits_file"]
    found = sorted(Path(cef_path).rglob(credits)) if cef_path and Path(cef_path).is_dir() else []
    if found:
        shutil.copy(found[0], out / credits)
        print(f"copied {credits} from {found[0].parent}")
    else:
        print(f"no {credits}: set CEF_PATH to the CEF distribution to copy Chromium's credits", file=sys.stderr)


if __name__ == "__main__":
    main()
