import { createStore } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import { installLanguageAtom, setLanguageAudioAtom } from "../../src/dictionaries/dispatch";
import { dictionariesViewAtom } from "../../src/dictionaries/state";
import {
  dictionaryLanguages,
  type CatalogPackSummary,
  type DictionaryInstallation,
} from "../../src/dictionaries/model";
import { bootstrapAtom } from "../../src/app/session";
import { libraryBootstrap } from "../support/state";

afterEach(() => {
  vi.unstubAllGlobals();
});

const corpus: CatalogPackSummary = {
  packType: "corpus",
  packId: "wiktionary-fr-fr",
  packRevision: "b".repeat(64),
  corpusLanguage: "fr",
  edition: "frwiktionary",
  dumpDate: "2026-09-01",
  monolingual: true,
  bilingualTargets: ["en"],
  licenses: ["CC-BY-SA-4.0"],
  installedBytes: 2_598_395_904,
  installed: true,
};

const streamed: CatalogPackSummary = {
  packType: "audio",
  packId: "wiktionary-fr-audio",
  packRevision: "c".repeat(64),
  corpusLanguage: "fr",
  delivery: "remote",
  recordings: 383_287,
  installedBytes: 67_108_864,
  installed: false,
};

const stored: CatalogPackSummary = {
  ...streamed,
  packRevision: "d".repeat(64),
  delivery: "bundled",
  installedBytes: 1_953_497_088,
};

function installation(pack: CatalogPackSummary): DictionaryInstallation {
  return {
    packType: pack.packType,
    packId: pack.packId,
    packRevision: pack.packRevision,
    corpusLanguage: pack.corpusLanguage,
    installedBytes: pack.installedBytes,
    delivery: pack.packType === "audio" ? pack.delivery : null,
  };
}

interface Host {
  /** Every request, as `METHOD path`. */
  readonly calls: string[];
  /** What each install asked for, in order. */
  readonly installs: { corpusLanguage: string; audio: string }[];
}

/** A host that completes every install at once, recording what it was asked to do. */
function host(packs: readonly CatalogPackSummary[], installations: DictionaryInstallation[]): Host {
  const calls: string[] = [];
  const installs: Host["installs"] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(async (input, init = {}) => {
      const path = input instanceof Request ? input.url : String(input);
      const method = init.method ?? "GET";
      calls.push(`${method} ${path}`);
      if (path === "/native/dictionary/catalog") {
        return Response.json({ status: "completed", catalogRevision: "r", packs });
      }
      if (path === "/native/dictionary/installations" && method === "POST") {
        installs.push(JSON.parse(init.body as string) as Host["installs"][number]);
        return Response.json({ status: "completed", operationId: "a".repeat(32) });
      }
      if (path.startsWith("/native/dictionary/operations/")) {
        return Response.json({
          status: "completed",
          operationId: "a".repeat(32),
          corpusLanguage: "fr",
          audio: installs.at(-1)?.audio ?? "streamed",
          phase: "completed",
          bytesCompleted: 1,
          bytesTotal: 1,
          failure: null,
        });
      }
      return Response.json({ status: "completed", installations });
    }),
  );
  return { calls, installs };
}

function store(packs: readonly CatalogPackSummary[], installations: DictionaryInstallation[]) {
  const created = createStore();
  created.set(bootstrapAtom, libraryBootstrap);
  created.set(dictionariesViewAtom, { status: "ready", packs: [...packs], installations });
  return created;
}

function french(packs: readonly CatalogPackSummary[]) {
  const [language] = dictionaryLanguages(packs);
  if (language === undefined) throw new Error("the language should be offered");
  return language;
}

test("installing a language is one request for the whole language", async () => {
  const packs = [{ ...corpus, installed: false }, streamed, stored];
  const asked = host(packs, []);
  const created = store(packs, []);

  await created.set(installLanguageAtom, french(packs));

  // The host installs the corpus and the pronunciations it comes with under one operation.
  expect(asked.installs).toStrictEqual([{ corpusLanguage: "fr", audio: "streamed" }]);
  expect(created.get(dictionariesViewAtom).status).toBe("ready");
});

test("finishing an install that stopped short asks for the same one language", async () => {
  const packs = [corpus, streamed, stored];
  const installations = [installation(corpus)];
  const asked = host(packs, installations);
  const created = store(packs, installations);

  await created.set(installLanguageAtom, french(packs));

  expect(asked.installs).toStrictEqual([{ corpusLanguage: "fr", audio: "streamed" }]);
});

/** Both deliveries are revisions of one pack, so installing one retires the other. */
test("switching delivery asks for the other delivery and removes nothing", async () => {
  const packs = [corpus, { ...streamed, installed: true }, stored];
  const installations = [installation(corpus), installation(streamed)];
  const asked = host(packs, installations);
  const created = store(packs, installations);

  await created.set(setLanguageAudioAtom, french(packs), "stored");

  expect(asked.installs).toStrictEqual([{ corpusLanguage: "fr", audio: "stored" }]);
  expect(asked.calls.some((call) => call.startsWith("DELETE"))).toBe(false);
});

test("a language that already plays its recordings that way is left alone", async () => {
  const packs = [corpus, { ...streamed, installed: true }, stored];
  const installations = [installation(corpus), installation(streamed)];
  const asked = host(packs, installations);
  const created = store(packs, installations);

  await created.set(setLanguageAudioAtom, french(packs), "streamed");

  expect(asked.installs).toStrictEqual([]);
});
