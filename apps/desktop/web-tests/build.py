#!/usr/bin/env python3
"""Inline harness.js and base.css into each page in src/, writing out/: one self-contained file per page, as an
artifact is. A page marks the spot with <!--HARNESS-->."""
import pathlib

here = pathlib.Path(__file__).parent
head = f"<style>{(here / 'base.css').read_text()}</style>\n<script>{(here / 'harness.js').read_text()}</script>"
out = here / "out"
out.mkdir(exist_ok=True)
for page in sorted((here / "src").glob("*.html")):
    (out / page.name).write_text(page.read_text().replace("<!--HARNESS-->", head))
    print(out / page.name)
