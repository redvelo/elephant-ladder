# W3C EPUB Test Corpus

`corpus.json` is the generated catalog of the W3C EPUB 3.4 reading-system tests at the
upstream commit it names: every test's identifier, package metadata, and the SHA-256 of
the EPUB built from its directory. The EPUBs themselves are not committed. They are built
by `scripts/build-w3c-corpus.mjs` into `$XDG_CACHE_HOME/elephant-ladder/w3c-epub-tests/<commit>/epubs/`
(default `~/.cache`), deterministically, from the upstream source archive at that commit,
with the upstream authored paths and bytes unchanged.

```sh
pnpm corpus:w3c          # fetch the pinned commit, build the EPUBs, regenerate the catalog
pnpm corpus:w3c:check    # rebuild and prove the committed catalog still matches
```

Source: <https://github.com/w3c/epub-tests>

License: [W3C Software and Document License](https://www.w3.org/Consortium/Legal/copyright-software)
