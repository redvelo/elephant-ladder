# Engine Support Probe

`elephant-engine-support.epub` is an owned fixture that asks a reading system's engine
what it supports, and shows the answers on the page as well as in one compact line per
group. It travels as an ordinary publication, so a platform with no harness — Windows,
iOS, macOS, or a third-party reading system — can be measured by opening it and reading
the result.

It declares one scripted XHTML document, one stylesheet, and one file in each image and
audio core media type. Nothing in it reaches outside the container. The source is under
`elephant-engine-support-src/`; the build is deterministic:

```sh
pnpm fixtures:engine-support          # rebuild from source
pnpm fixtures:engine-support:check    # prove the committed EPUB matches its source
pnpm probe:engine-support             # run it and print the engine's answers
pnpm probe:engine-support --android   # the same on the attached device
```

What it asks:

- which `-epub-` prefixed properties the engine aliases by itself, and which of the
  properties they stand for it implements;
- whether it decodes a file in each of PNG, JPEG, GIF, WebP, AVIF and JPEG XL;
- what it claims for MP3, AAC and Opus, and whether it reads each file's metadata;
- what `navigator.epubReadingSystem` reports, including the required feature strings.

The media files are four-pixel images and eighty-millisecond silences, generated once
with Pillow, `avifenc`, `cjxl` and `ffmpeg`, and committed as fixture source.

| File | Bytes | SHA-256 |
| --- | --- | --- |
| `elephant-engine-support.epub` | 7911 | `5920f559cb3073256cda146212a2bb47a7174311cf8359f11dc38f0ebe921c52` |
