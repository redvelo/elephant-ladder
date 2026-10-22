import { afterEach, expect, test } from "vitest";
import {
  activeOperation,
  cancelDictionaryOperation,
  deleteDictionaryInstallation,
  DictionaryCatalogUnavailable,
  fetchDictionaryCatalog,
  fetchDictionaryInstallations,
  fetchDictionaryOperations,
  pollDictionaryOperation,
  startDictionaryInstall,
  watchDictionaryOperations,
} from "../../src/dictionaries/requests";
import { dictionaryLanguages, installedLanguages } from "../../src/dictionaries/model";
import { formatBytes } from "../../src/lib/format";

const revision = "a".repeat(64);
const context = { sessionToken: "token", libraryDocumentId: "document" };

const pack = {
  packType: "corpus",
  packId: "wiktionary-fr-fr",
  packRevision: revision,
  corpusLanguage: "fr",
  edition: "frwiktionary",
  dumpDate: "2026-09-01",
  monolingual: true,
  bilingualTargets: ["en"],
  licenses: ["CC-BY-SA-4.0"],
  installedBytes: 2_598_395_904,
  installed: false,
};

const previousFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = previousFetch;
});

function capture(response: () => Response) {
  const calls: { input: string; method: string; body: unknown; headers: unknown }[] = [];
  globalThis.fetch = async (input, init = {}) => {
    calls.push({
      input: input instanceof Request ? input.url : String(input),
      method: String(init.method),
      body: init.body === undefined ? null : JSON.parse(init.body as string),
      headers: init.headers,
    });
    return response();
  };
  return calls;
}

test("the catalog request carries the session and library document identity", async () => {
  const calls = capture(() =>
    Response.json({ status: "completed", catalogRevision: "2026-09-18.1", packs: [pack] }),
  );
  const catalog = await fetchDictionaryCatalog(context);
  expect(catalog.packs[0]?.corpusLanguage).toBe("fr");
  expect(calls).toStrictEqual([
    {
      input: "/native/dictionary/catalog",
      method: "GET",
      body: null,
      headers: {
        Authorization: "Bearer token",
        "X-Elephant-Library-Document": "document",
      },
    },
  ]);
});

test("a catalog missing required pack facts is rejected", async () => {
  const { dumpDate: _dumpDate, ...incomplete } = pack;
  capture(() => Response.json({ status: "completed", catalogRevision: "r", packs: [incomplete] }));
  await expect(fetchDictionaryCatalog(context)).rejects.toThrow("invalid");
});

test("pronunciation collections are listed beside corpora", async () => {
  const audio = {
    packType: "audio",
    packId: "wiktionary-fr-audio",
    packRevision: "c".repeat(64),
    corpusLanguage: "fr",
    delivery: "remote",
    recordings: 383_795,
    installedBytes: 64_000_000,
    installed: false,
  };
  capture(() => Response.json({ status: "completed", catalogRevision: "r", packs: [pack, audio] }));
  const catalog = await fetchDictionaryCatalog(context);
  expect(catalog.packs.map((listed) => listed.packType)).toStrictEqual(["corpus", "audio"]);

  capture(() =>
    Response.json({
      status: "completed",
      catalogRevision: "r",
      packs: [{ ...audio, delivery: "somewhere" }],
    }),
  );
  await expect(fetchDictionaryCatalog(context)).rejects.toThrow("invalid");
});

test("a catalog the device cannot reach is its own outcome, not a failure", async () => {
  capture(() =>
    Response.json({ status: "unavailable", reason: "catalog-unavailable" }, { status: 503 }),
  );
  await expect(fetchDictionaryCatalog(context)).rejects.toThrow(DictionaryCatalogUnavailable);
});

test("installations report how their recordings are delivered", async () => {
  const installation = {
    packType: "audio",
    packId: "wiktionary-fr-audio",
    packRevision: "c".repeat(64),
    corpusLanguage: "fr",
    installedBytes: 64_000_000,
    delivery: "remote",
  };
  capture(() =>
    Response.json({
      status: "completed",
      installations: [
        { ...pack, packType: "corpus", delivery: null, installed: undefined },
        installation,
      ],
    }),
  );
  const installations = await fetchDictionaryInstallations(context);
  expect(installations.map((listed) => listed.delivery)).toStrictEqual([null, "remote"]);

  capture(() =>
    Response.json({
      status: "completed",
      installations: [{ ...installation, delivery: "elsewhere" }],
    }),
  );
  await expect(fetchDictionaryInstallations(context)).rejects.toThrow("invalid");
});

test("installing names the language and where its recordings play from", async () => {
  const calls = capture(() => Response.json({ status: "completed", operationId: "b".repeat(32) }));
  const operationId = await startDictionaryInstall(context, "fr", "stored");
  expect(operationId).toBe("b".repeat(32));
  expect(calls[0]?.method).toBe("POST");
  expect(calls[0]?.body).toStrictEqual({ corpusLanguage: "fr", audio: "stored" });
});

test("polling reports progress until a terminal phase", async () => {
  const phases = ["downloading", "verifying", "completed"] as const;
  let call = 0;
  globalThis.fetch = async () => {
    const phase = phases[Math.min(call, phases.length - 1)];
    call += 1;
    return Response.json({
      status: "completed",
      operationId: "b".repeat(32),
      corpusLanguage: "fr",
      audio: "streamed",
      phase,
      bytesCompleted: call * 100,
      bytesTotal: 300,
      failure: null,
    });
  };
  const seen: string[] = [];
  const finished = await pollDictionaryOperation(context, "b".repeat(32), {
    wait: async () => {},
    onProgress: (operation) => seen.push(operation.phase),
  });
  expect(seen).toStrictEqual(["downloading", "verifying", "completed"]);
  expect(finished.phase).toBe("completed");
});

test("polling stops when the caller aborts", async () => {
  const controller = new AbortController();
  globalThis.fetch = async () =>
    Response.json({
      status: "completed",
      operationId: "b".repeat(32),
      corpusLanguage: "fr",
      audio: "streamed",
      phase: "downloading",
      bytesCompleted: 1,
      bytesTotal: 2,
      failure: null,
    });
  controller.abort();
  const finished = await pollDictionaryOperation(
    { ...context, signal: controller.signal },
    "b".repeat(32),
    { wait: async () => {} },
  );
  expect(finished.phase).toBe("downloading");
});

test("a stale library document surfaces the host message", async () => {
  capture(() =>
    Response.json({ status: "failed", message: "stale library document" }, { status: 409 }),
  );
  await expect(fetchDictionaryInstallations(context)).rejects.toThrow("stale library document");
});

test("cancel and delete address operations and revisions by identity", async () => {
  const calls = capture(() => Response.json({ status: "completed" }));
  await cancelDictionaryOperation(context, "b".repeat(32));
  await deleteDictionaryInstallation(context, pack.packId, revision);
  expect(calls.map((call) => `${call.method} ${call.input}`)).toStrictEqual([
    `DELETE /native/dictionary/operations/${"b".repeat(32)}`,
    `DELETE /native/dictionary/installations/${pack.packId}/${revision}`,
  ]);
});

test("the operations listing surfaces the running install", async () => {
  const running = {
    operationId: "b".repeat(32),
    corpusLanguage: "fr",
    audio: "streamed",
    phase: "downloading",
    bytesCompleted: 10,
    bytesTotal: 100,
    failure: null,
  };
  const calls = capture(() =>
    Response.json({
      status: "completed",
      operations: [{ ...running, phase: "completed", operationId: "d".repeat(32) }, running],
    }),
  );
  const operations = await fetchDictionaryOperations(context);
  expect(operations).toHaveLength(2);
  expect(activeOperation(operations)?.operationId).toBe("b".repeat(32));
  expect(calls[0]?.input).toBe("/native/dictionary/operations");
});

test("an operations listing with a bad phase is rejected", async () => {
  capture(() =>
    Response.json({
      status: "completed",
      operations: [
        {
          operationId: "b".repeat(32),
          corpusLanguage: "fr",
          audio: "streamed",
          phase: "elsewhere",
          bytesCompleted: 0,
          bytesTotal: 0,
          failure: null,
        },
      ],
    }),
  );
  await expect(fetchDictionaryOperations(context)).rejects.toThrow("invalid");
});

test("no running install means nothing to report", () => {
  expect(
    activeOperation([
      {
        operationId: "d".repeat(32),
        corpusLanguage: "fr",
        audio: "streamed",
        phase: "interrupted",
        bytesCompleted: 5,
        bytesTotal: 10,
        failure: null,
      },
    ]),
  ).toBeNull();
});

test("watching follows the install to its end and then stops", async () => {
  const phases = ["downloading", "downloading", "installing", "completed"] as const;
  let call = 0;
  globalThis.fetch = async () => {
    const phase = phases[Math.min(call, phases.length - 1)];
    call += 1;
    return Response.json({
      status: "completed",
      operations: [
        {
          operationId: "b".repeat(32),
          corpusLanguage: "fr",
          audio: "streamed",
          phase,
          bytesCompleted: call * 10,
          bytesTotal: 100,
          failure: null,
        },
      ],
    });
  };
  const seen: (string | null)[] = [];
  await watchDictionaryOperations(context, {
    wait: async () => {},
    onChange: (operation) => seen.push(operation?.phase ?? null),
  });
  // Every progress step is reported, and the terminal phase ends the watch.
  expect(seen).toStrictEqual(["downloading", "downloading", "installing", null]);
  expect(call).toBe(4);
});

test("watching stops when the library goes away", async () => {
  const controller = new AbortController();
  let call = 0;
  globalThis.fetch = async () => {
    call += 1;
    controller.abort();
    return Response.json({
      status: "completed",
      operations: [
        {
          operationId: "b".repeat(32),
          corpusLanguage: "fr",
          audio: "streamed",
          phase: "downloading",
          bytesCompleted: 1,
          bytesTotal: 2,
          failure: null,
        },
      ],
    });
  };
  await watchDictionaryOperations(
    { ...context, signal: controller.signal },
    { wait: async () => {}, onChange: () => {} },
  );
  expect(call).toBe(1);
});

test("a language offers its corpus with streamed and stored pronunciations", () => {
  const audio = {
    packType: "audio" as const,
    packId: "wiktionary-fr-audio",
    corpusLanguage: "fr",
    recordings: 383_287,
    installed: false,
  };
  const languages = dictionaryLanguages([
    { ...pack, packType: "corpus" as const },
    {
      ...audio,
      packRevision: "c".repeat(64),
      delivery: "remote" as const,
      installedBytes: 67_108_864,
    },
    {
      ...audio,
      packRevision: "d".repeat(64),
      delivery: "bundled" as const,
      installedBytes: 1_953_497_088,
    },
    {
      ...audio,
      packId: "wiktionary-de-audio",
      packRevision: "e".repeat(64),
      corpusLanguage: "de",
      delivery: "remote" as const,
      installedBytes: 1,
    },
  ] as never);
  // Pronunciations with no corpus to belong to are not offered on their own.
  expect(languages.map((language) => language.corpusLanguage)).toStrictEqual(["fr"]);
  const [french] = languages;
  expect(french?.audio).toBe("none");
  expect(french?.complete).toBe(false);
  expect(french?.streamed?.delivery).toBe("remote");
  expect(french?.stored?.delivery).toBe("bundled");
  // The offered size is the corpus plus the streamed index, not the stored recordings.
  expect(french?.installedBytes).toBe(pack.installedBytes + 67_108_864);
});

test("an installed stored collection is what the language reports", () => {
  const [french] = dictionaryLanguages([
    { ...pack, packType: "corpus" as const, installed: true },
    {
      packType: "audio" as const,
      packId: "wiktionary-fr-audio",
      packRevision: "d".repeat(64),
      corpusLanguage: "fr",
      delivery: "bundled" as const,
      recordings: 383_285,
      installedBytes: 1_953_497_088,
      installed: true,
    },
  ] as never);
  expect(french?.audio).toBe("stored");
  expect(french?.installed).toBe(true);
  expect(french?.complete).toBe(true);
});

test("a corpus installed without its pronunciations is installed but incomplete", () => {
  const [french] = dictionaryLanguages([
    { ...pack, packType: "corpus" as const, installed: true },
    {
      packType: "audio" as const,
      packId: "wiktionary-fr-audio",
      packRevision: "c".repeat(64),
      corpusLanguage: "fr",
      delivery: "remote" as const,
      recordings: 383_287,
      installedBytes: 67_108_864,
      installed: false,
    },
  ] as never);
  expect(french?.installed).toBe(true);
  expect(french?.complete).toBe(false);

  // A language the catalog offers no pronunciations for is complete with its corpus.
  const [german] = dictionaryLanguages([
    { ...pack, packType: "corpus" as const, corpusLanguage: "de", installed: true },
  ] as never);
  expect(german?.complete).toBe(true);
});

test("installed languages are what the device holds when the catalog is unreachable", () => {
  const audio = {
    packType: "audio" as const,
    packId: "wiktionary-fr-audio",
    packRevision: "c".repeat(64),
    corpusLanguage: "fr",
    installedBytes: 64_000_000,
    delivery: "remote" as const,
  };
  const corpus = {
    packType: "corpus" as const,
    packId: "wiktionary-fr-fr",
    packRevision: revision,
    corpusLanguage: "fr",
    installedBytes: 2_598_395_904,
    delivery: null,
  };
  const languages = installedLanguages([
    { ...corpus, packId: "wiktionary-en-en", corpusLanguage: "en" },
    corpus,
    audio,
    // Recordings whose corpus is gone are no language anyone can read.
    { ...audio, packId: "wiktionary-de-audio", corpusLanguage: "de" },
  ]);
  expect(languages.map((language) => language.corpusLanguage)).toStrictEqual(["en", "fr"]);
  expect(languages[1]).toStrictEqual({
    corpusLanguage: "fr",
    installedBytes: corpus.installedBytes + audio.installedBytes,
    audio: "streamed",
  });
  expect(languages[0]?.audio).toBe("none");
  expect(installedLanguages([corpus, { ...audio, delivery: "bundled" }])[0]?.audio).toBe("stored");
  // A collection the host could not open reports no delivery and plays nothing.
  expect(installedLanguages([corpus, { ...audio, delivery: null }])[0]?.audio).toBe("none");
});

test("sizes read as short units", () => {
  expect(formatBytes(0)).toBe("0 B");
  expect(formatBytes(2_598_395_904)).toBe("2.4 GiB");
});
