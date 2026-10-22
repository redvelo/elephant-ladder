#!/usr/bin/env python3
"""Pack src/ into one-small-step.epub: `mimetype` first and stored, media stored, the rest deflated."""
import pathlib, zipfile
here = pathlib.Path(__file__).resolve().parent
src, out = here / "src", here / "one-small-step.epub"
with zipfile.ZipFile(out, "w") as z:
    z.writestr(zipfile.ZipInfo("mimetype"), "application/epub+zip", compress_type=zipfile.ZIP_STORED)
    for f in sorted(p for p in src.rglob("*") if p.is_file()):
        stored = f.suffix in (".webm", ".jpg")
        z.write(f, f.relative_to(src).as_posix(), compress_type=zipfile.ZIP_STORED if stored else zipfile.ZIP_DEFLATED)
print(f"wrote {out} ({out.stat().st_size // 1024} KB)")
