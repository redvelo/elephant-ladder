# Active Publication Fixtures

These EPUB files are committed test inputs. They are not application assets and are
not included in production packages.

## Static Fixed Layout

`static-fxl-ltr.epub`, `static-fxl-rtl.epub`, and `static-fxl-mixed-svg.epub`
are owned synthetic EPUB 3 fixtures with no third-party assets, authored scripts,
event handlers, or scripted manifest declarations. Source and deterministic archive
generation are in `build-static-fxl.mjs` in this directory. Run that file with Node
to generate, or with `--check` to verify the archives.

Each has a 300 x 400 centered first page followed by an explicitly sided pair.
XHTML LTR and RTL cases reverse the pair's physical sides. The mixed LTR case uses
XHTML as primary and a static root SVG as companion. Midlines extend to both page
edges to expose mismatched seams. The `static-fxl-v1` contract records two trusted
parent/runtime snapshots and eight checks: visible occurrence identity, adapters,
placement, and contain/seam geometry for each phase. No authored report or
publication snapshot script is added; only normal trusted content-agent preparation
is used. The runner does not claim native measurement or cache availability.

| Fixture ID / file | Size (bytes) | SHA-256 | BLAKE3 |
| --- | ---: | --- | --- |
| `static-fxl-ltr.epub` | 2319 | `7992cfb780207da629736cbc83045317e51eec0022772ca4740d239f04e5d86c` | `2deec1b53dc04bffa8f6a9b97cc06a7754f0a502258bb6997a29fb3e590263d0` |
| `static-fxl-rtl.epub` | 2318 | `03e6a0e4a282ab72c6a8dc9b769a795710e52c086ed8d9de63bb851b4ed67735` | `2185a2ce98eedab16e02ba7f56ecf52ee77d0f1be22039efe64af324a3c57211` |
| `static-fxl-mixed-svg.epub` | 2244 | `818ea3b80955de4da3d9e48f893205c4dc2fb2883127bff35e63432fe293da4c` | `42c9a9d713ab874f28d49c4636380c7616496d821771422a8492bf32cec2833d` |

First-occurrence side override and packaged WebView execution remain separate
qualification coverage; these fixtures do not assert them.

## Elephant Active Probe

`elephant-active-probe.epub` and `elephant-active-probe-twin.epub` are owned and
maintained by the Elephant Ladder project. They contain no third-party assets. Their
source is checked in under `elephant-active-probe-src/`; the 44-byte WAV is decoded
from its checked-in base64 representation.

Generate both deterministic EPUB 3.4 archives with:

```sh
node scripts/build-active-probe-fixtures.mjs
```

Verify the committed archives, metadata below, EPUB ZIP invariants, and twin-resource
contract with:

```sh
node scripts/build-active-probe-fixtures.mjs --check
```

The twin changes only `dc:identifier` and `dc:title` in `EPUB/package.opf`. It keeps
all archive paths, resource bytes, localStorage keys, and IndexedDB names and keys
identical so hosts can test publication-principal isolation.

| Local file | Size (bytes) | SHA-256 |
| --- | ---: | --- |
| `elephant-active-probe.epub` | 14036 | `52882e792e632bd51590b1e50421b78d9dc3ae3ce8c367852e18c2606570ab24` |
| `elephant-active-probe-twin.epub` | 14041 | `fda27108e9bc17fcc32c223465541a66abc1219e4c4a0147d48de4422a1b6030` |
| `elephant-container-constrained.epub` | 3205 | `2371f81ec4a599e04305f18b1bb85b089abb322bd9460e53046c6f7aeced5b34` |
| `elephant-remote-policy.epub` | 3217 | `e5120c80e46e8145d53bd182ed4c874986e0a7727aa1437201c40ca39acc6ef0` |

Reports use `type: "elephant-active-probe-result-v1"`, one of the scenario names
below, and one of the bounded outcomes `passed`, `failed`, `blocked`, or `unsupported`.
The normal framed-host expectation is `blocked` for `remote-fetch`,
`service-worker-registration`, `reader-isolation`, and `popup`; supported functional
probes report `passed`.

Scenarios (27): `classic-script`, `document-module-static-import`, `document-module-dynamic-import`, `relative-fetch`, `root-relative-fetch`, `remote-fetch`, `dedicated-module-worker`, `dedicated-worker-teardown`, `shared-worker`, `service-worker-registration`, `stylesheet`, `image`, `scripted-svg`, `wav-range`, `local-storage`, `indexed-db`, `cookie`, `document-domain-relaxation`, `reader-isolation`, `native-capability-isolation`, `popup`, `peer-local-storage`, `peer-indexed-db`, `peer-cookie`, `peer-worker-teardown`, `probe-complete`, `peer-complete`.

The teardown scenarios keep a dedicated worker alive in each probe document and require
its bounded same-origin heartbeat to disappear after Navigator commits the reload and
peer-document transitions. Shared-worker construction records `passed`, `blocked`, or
`unsupported`; it does not by itself claim a platform teardown primitive.

The cookie scenario records either host-only persistence or coherent browser-level
unavailability and rejects a visible `Domain=localhost` widening attempt. The
document-domain scenario records the exact assignment result and before/after domain and
origin values; packaged runs, rather than the fixture source, establish browser behavior.

`elephant-remote-policy.epub` is an owned deterministic EPUB 3.0 fixture with source
under `elephant-remote-policy-src/`. Its content document declares both `scripted` and
`remote-resources` and attempts HTTPS and HTTP fetches, an HTTPS image, stylesheet,
script, audio resource and frame, and WSS and WS connections against `httpbingo.org`. The
offline contract requires exact browser CSP violations. Public endpoint availability is
relevant only to the future allowed-network contract.

`elephant-container-constrained.epub` is an owned synthetic fixture with source under
`elephant-container-constrained-src/`. Its static local child frame attempts to modify
its foreground parent, containing iframe, reader DOM, credentials, and a native route.
It reports through a strict opaque-child-to-foreground `postMessage` relay.

## W3C EPUB Tests

The W3C EPUB test corpus is no longer copied here. `tests/fixtures/w3c-corpus/corpus.json`
catalogs every upstream test at a pinned commit, and `scripts/build-w3c-corpus.mjs` builds
the EPUBs into the user's cache; see `tests/fixtures/w3c-corpus/NOTICE.md`.
## johnfactotum/epub-test

Source: <https://github.com/johnfactotum/epub-test>

Pinned revision: `c27a276981de89526814db2841f9be57bef029e2`

License: [CC0 1.0 Universal](https://creativecommons.org/publicdomain/zero/1.0/)

| Local file | Upstream file | SHA-256 |
| --- | --- | --- |
| `johnfactotum-epub-test.epub` | `epub-test.epub` | `e67e852ca530ef85780a130121469adce8e64f15104fa717ad9ceae07de1a2a4` |

This fixture intentionally probes unsafe and external behavior, including `file:` URLs,
Node access, remote requests, forms, frames, and top-level navigation. It omits required
`scripted` and `remote-resources` package declarations, so declaration absence must not be
treated as evidence that the publication is inactive.

## Integrity

To check the committed files against this notice, run:

```sh
sha256sum tests/fixtures/active-publications/*.epub
```
