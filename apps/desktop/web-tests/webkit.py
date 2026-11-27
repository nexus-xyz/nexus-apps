#!/usr/bin/env python3
"""The suite in WebKitGTK (WKWebView on macOS is its sibling).
Usage: webkit.py <base url> <page>... ; prints each page's title (its score) and saves shots/webkit/<page>.png."""
import sys, pathlib, gi
gi.require_version("Gtk", "4.0"); gi.require_version("WebKit", "6.0")
from gi.repository import Gtk, WebKit, GLib, Gdk

base, pages = sys.argv[1], sys.argv[2:]
out = pathlib.Path(__file__).parent / "shots/webkit"; out.mkdir(parents=True, exist_ok=True)
app = Gtk.Application()

def run(app):
    win = Gtk.ApplicationWindow(application=app, default_width=470, default_height=1300)
    view = WebKit.WebView(); win.set_child(view); win.present()
    s = view.get_settings(); s.set_enable_webgl(True); s.set_hardware_acceleration_policy(WebKit.HardwareAccelerationPolicy.ALWAYS)
    queue = list(pages)
    def shot(name):
        def done(v, res):
            try: v.get_snapshot_finish(res).save_to_png(str(out / f"{name}.png"))
            except Exception as e: print("snapshot failed", e)
            print(f"webkit {name}: {view.get_title()}", flush=True); nxt()
        view.get_snapshot(WebKit.SnapshotRegion.VISIBLE, WebKit.SnapshotOptions.NONE, None, done)
        return False
    def nxt():
        if not queue: app.quit(); return
        name = queue.pop(0); view.load_uri(f"{base}/{name}.html"); GLib.timeout_add_seconds(12, shot, name)
    nxt()
app.connect("activate", run); app.run([])
