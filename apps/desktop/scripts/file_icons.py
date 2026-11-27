#!/usr/bin/env python3
"""Regenerate the file-type icons: `assets/file_icons/` and `src/file_icons/table.rs`.

The icons and their associations are Material Icon Theme's (MIT), from its npm package's built manifest
(`dist/material-icons.json`, the VS Code file icon theme). VS Code reaches some icons through a language id (`rust`,
`python`), so the language layer comes from VS Code's built-in language contributions (MIT): the `extensions` and
`filenames` of each `extensions/*/package.json`.

    python3 -I scripts/file_icons.py                      # download the pinned versions, then generate
    python3 -I scripts/file_icons.py --material DIR --vscode DIR   # use unpacked copies instead

Run it from desktop-gpui. Standard library only. Needs `npm` and `git` when it downloads.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

MATERIAL_VERSION = "5.40.0"
VSCODE_REPO = "https://github.com/microsoft/vscode.git"
VSCODE_COMMIT = "89f7e47811dedebb4a69e8ca59416ba9e7fde88c"

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets" / "file_icons"
TABLE = ROOT / "src" / "file_icons" / "table.rs"
LICENSES = ROOT / "licenses" / "third-party"


def download_material(into: Path) -> Path:
    subprocess.run(
        ["npm", "pack", f"material-icon-theme@{MATERIAL_VERSION}", "--silent"], cwd=into, check=True, capture_output=True
    )
    tarball = next(into.glob("material-icon-theme-*.tgz"))
    shutil.unpack_archive(tarball, into)
    return into / "package"


def download_vscode(into: Path) -> Path:
    repo = into / "vscode"
    git = ["git", "-C", str(repo)]
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run([*git, "remote", "add", "origin", VSCODE_REPO], check=True)
    subprocess.run([*git, "sparse-checkout", "set", "--no-cone", "/extensions/*/package.json", "/LICENSE.txt"], check=True)
    subprocess.run([*git, "fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", VSCODE_COMMIT], check=True)
    subprocess.run([*git, "checkout", "-q", "FETCH_HEAD"], check=True)
    return repo


def vscode_languages(vscode: Path) -> tuple[dict[str, str], dict[str, str]]:
    """VS Code's built-in file names and extensions, lowercased, each to its language id. The first claim wins."""
    names: dict[str, str] = {}
    extensions: dict[str, str] = {}
    for manifest in sorted((vscode / "extensions").glob("*/package.json")):
        contributes = json.loads(manifest.read_text(encoding="utf-8")).get("contributes", {})
        for language in contributes.get("languages", []):
            for name in language.get("filenames", []):
                names.setdefault(name.lower(), language["id"])
            for extension in language.get("extensions", []):
                extensions.setdefault(extension.lower().lstrip("."), language["id"])
    return names, extensions


def lowered(table: dict[str, str]) -> dict[str, str]:
    """VS Code matches file and folder names in any case. The first spelling of a name wins."""
    out: dict[str, str] = {}
    for key, icon in table.items():
        out.setdefault(key.lower(), icon)
    return out


def rust_str(text: str) -> str:
    return json.dumps(text, ensure_ascii=False)


def generate(material: Path, vscode: Path) -> None:
    theme = json.loads((material / "dist" / "material-icons.json").read_text(encoding="utf-8"))
    light = theme["light"]

    def pairs(kind: str) -> dict[str, tuple[str, str]]:
        """One association table, lowercased: each key to its dark icon and the icon a light theme shows. As in VS
        Code, a key that the light table does not list keeps its dark icon."""
        lit = lowered(light.get(kind, {}))
        return {key: (icon, lit.get(key, icon)) for key, icon in lowered(theme[kind]).items()}

    file_names = pairs("fileNames")
    file_extensions = pairs("fileExtensions")
    folder_names = pairs("folderNames")
    folder_open = pairs("folderNamesExpanded")
    language_icons = {lang: (icon, light["languageIds"].get(lang, icon)) for lang, icon in theme["languageIds"].items()}
    lang_names, lang_extensions = vscode_languages(vscode)
    language_names = {name: language_icons[lang] for name, lang in lang_names.items() if lang in language_icons}
    language_extensions = {ext: language_icons[lang] for ext, lang in lang_extensions.items() if lang in language_icons}
    if set(folder_open) != set(folder_names):
        sys.exit("folderNames and folderNamesExpanded name different folders")

    used = {theme["file"], theme["folder"], theme["folderExpanded"]}
    for table in (file_names, file_extensions, language_names, language_extensions, folder_names, folder_open):
        for dark, lit in table.values():
            used |= {dark, lit}

    # Sorted by asset path, which `load` searches: `folder-open.svg` comes before `folder.svg`.
    icons = sorted(used, key=lambda icon: f"{icon}.svg")
    # A recoloured clone (`advpl-include`) is its own file, `<name>.clone.svg`. Each icon lands as `<name>.svg`.
    sources = {}
    for icon in icons:
        path = theme["iconDefinitions"][icon]["iconPath"]
        if path not in (f"./../icons/{icon}.svg", f"./../icons/{icon}.clone.svg"):
            sys.exit(f"icon {icon!r} is at {path!r}")
        sources[icon] = material / "icons" / Path(path).name

    if ASSETS.exists():
        shutil.rmtree(ASSETS)
    ASSETS.mkdir(parents=True)
    for icon in icons:
        shutil.copyfile(sources[icon], ASSETS / f"{icon}.svg")
    LICENSES.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(material / "LICENSE", LICENSES / "material-icon-theme-LICENSE")
    shutil.copyfile(vscode / "LICENSE.txt", LICENSES / "vscode-LICENSE")

    index = {icon: n for n, icon in enumerate(icons)}
    # The resolver tries a file's or a folder's `parent/name` and its name, so a key holds one `/` at most.
    deep = [key for key in [*file_names, *folder_names] if key.count("/") > 1]
    if deep:
        sys.exit(f"keys with more than one parent: {deep[:5]}")
    out = [
        f"// @generated by scripts/file_icons.py from material-icon-theme {MATERIAL_VERSION} (MIT) and VS Code's",
        f"// built-in languages at {VSCODE_COMMIT[:12]} (MIT). Do not edit it by hand: run the script.",
        "",
        "use super::Pair;",
        "",
        "/// Each icon's asset path and its SVG, sorted by path.",
        "pub(super) const ICONS: &[(&str, &[u8])] = &[",
    ]
    for icon in icons:
        out.append(f'    ("file_icons/{icon}.svg", include_bytes!("../../assets/file_icons/{icon}.svg")),')
    out += ["];", ""]

    def pair(icons: tuple[str, str]) -> str:
        return f"Pair {{ dark: {index[icons[0]]}, light: {index[icons[1]]} }}"

    def table(doc: str, name: str, entries: dict[str, tuple[str, str]]) -> None:
        out.append(f"/// {doc}")
        out.append(f"pub(super) const {name}: &[(&str, Pair)] = &[")
        out.extend(f"    ({rust_str(key)}, {pair(entries[key])})," for key in sorted(entries))
        out.extend(["];", ""])

    table("A whole file name, lowercased, or `parent/name`: its icons.", "FILE_NAMES", file_names)
    table("An extension without its first dot (`rs`, `d.ts`), lowercased: its icons.", "FILE_EXTENSIONS", file_extensions)
    table("A file name a VS Code language claims (`dockerfile`), lowercased.", "LANGUAGE_NAMES", language_names)
    table("An extension a VS Code language claims, lowercased.", "LANGUAGE_EXTENSIONS", language_extensions)
    out.append("/// A folder name, lowercased, or `parent/name`: its closed icons and its open ones.")
    out.append("pub(super) const FOLDER_NAMES: &[(&str, Pair, Pair)] = &[")
    out.extend(
        f"    ({rust_str(key)}, {pair(folder_names[key])}, {pair(folder_open[key])}),"
        for key in sorted(folder_names)
    )
    out += ["];", ""]
    file, folder, folder_expanded = theme["file"], theme["folder"], theme["folderExpanded"]
    out += [
        "/// The icons for a file or a folder that nothing names.",
        f"pub(super) const FILE: Pair = {pair((file, file))};",
        f"pub(super) const FOLDER: Pair = {pair((folder, folder))};",
        f"pub(super) const FOLDER_OPEN: Pair = {pair((folder_expanded, folder_expanded))};",
        "",
    ]
    TABLE.parent.mkdir(parents=True, exist_ok=True)
    TABLE.write_text("\n".join(out), encoding="utf-8")
    print(f"{len(icons)} icons, {len(file_names)} names, {len(file_extensions)} extensions, "
          f"{len(language_names)} + {len(language_extensions)} language entries, {len(folder_names)} folders")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--material", type=Path, help="an unpacked material-icon-theme npm package")
    parser.add_argument("--vscode", type=Path, help="a VS Code checkout with extensions/*/package.json")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as scratch:
        material = args.material or download_material(Path(scratch))
        vscode = args.vscode or download_vscode(Path(scratch))
        generate(material, vscode)


if __name__ == "__main__":
    main()
