# Media Overlay Fixtures

These EPUB files are committed test inputs. They are not application assets and are
not included in production packages.

## IDPF/W3C EPUB 3 Samples

Source: <https://github.com/IDPF/epub3-samples>

Pinned release: `20230704`

License: [CC-BY-SA 3.0](http://creativecommons.org/licenses/by-sa/3.0/)

| Local file | Upstream artifact | SHA-256 |
| --- | --- | --- |
| `moby-dick-mo.epub` | [`moby-dick-mo.epub`](https://github.com/IDPF/epub3-samples/releases/download/20230704/moby-dick-mo.epub) | `749a9de094c26bc79351722a2bf320845dc5f6af2d5bf41a96d720d723b29a4c` |

This is the same committed artifact used by EPUB Navigator's corpus fixtures
(`navigator/tests/fixtures/epub3-samples/moby-dick-mo.epub`), kept byte-identical so
host and reader evidence describe the same publication. Its package is at
`OPS/package.opf`; it declares two Media Overlay SMIL resources
(`OPS/chapter_001_overlay.smil`, `OPS/chapter_002_overlay.smil`) referencing
DEFLATE-compressed MP4 audio under `OPS/audio/`. Compressed audio storage means this
fixture only exercises the `MediaSidecar` backing path, never direct archive-extent
slicing.

`moby-dick-excerpt.m4a` is derived from `moby-dick-mo.epub` under the same license: the first
40 seconds of `OPS/audio/mobydick_001_002_melville.mp4`, stream-copied without re-encoding:

```sh
unzip -p tests/fixtures/media-overlays/moby-dick-mo.epub OPS/audio/mobydick_001_002_melville.mp4 > moby.mp4
ffmpeg -i moby.mp4 -t 40 -c copy -movflags +faststart tests/fixtures/media-overlays/moby-dick-excerpt.m4a
```

`moby-dick-excerpt.mp3` is derived from `moby-dick-excerpt.m4a` under the same license: the first
6 seconds, re-encoded to 32 kbit/s mono MP3 without metadata:

```sh
ffmpeg -i tests/fixtures/media-overlays/moby-dick-excerpt.m4a -t 6 -ac 1 -ar 22050 -c:a libmp3lame -b:a 32k -map_metadata -1 -write_xing 0 -id3v2_version 0 tests/fixtures/media-overlays/moby-dick-excerpt.mp3
```

`moby-dick-excerpt-opus.mp4` is derived from `moby-dick-excerpt.m4a` under the same license:
the first 6 seconds as Opus in an MP4 container, which EPUB 3.4 added as a core media type
and which no engine this application runs on decodes:

```sh
ffmpeg -i tests/fixtures/media-overlays/moby-dick-excerpt.m4a -t 6 -ac 1 -ar 48000 -c:a libopus -b:a 24k -map_metadata -1 tests/fixtures/media-overlays/moby-dick-excerpt-opus.mp4
```

## Integrity

To check the committed EPUB against this notice, run:

```sh
sha256sum tests/fixtures/media-overlays/*.epub
```
