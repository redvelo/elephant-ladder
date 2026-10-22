import { expect, test, vi } from "vitest";
import { nativeRequest } from "../../src/host/requests";

function stub() {
  return vi.fn<typeof fetch>().mockResolvedValue(new Response(null, { status: 204 }));
}

test("a request carries the session token and the authority the host validates", async () => {
  const fetcher = stub();
  await nativeRequest(
    "/native/publications",
    {
      sessionToken: "secret",
      libraryDocumentId: "document",
      readerSessionId: "session",
      trusted: true,
    },
    { fetcher },
  );
  expect(fetcher).toHaveBeenCalledExactlyOnceWith("/native/publications", {
    cache: "no-store",
    headers: {
      Authorization: "Bearer secret",
      "X-Elephant-Request": "trusted-reader",
      "X-Elephant-Library-Document": "document",
      "X-Elephant-Reader-Session": "session",
    },
  });
});

test("an authority sends only the headers it names", async () => {
  const fetcher = stub();
  await nativeRequest("/native/storage/usage", { sessionToken: "secret" }, { fetcher });
  const [, init] = fetcher.mock.calls[0]!;
  expect(init?.headers).toEqual({ Authorization: "Bearer secret" });
});

test("a JSON body sets its own content type, and an explicit one wins", async () => {
  const fetcher = stub();
  await nativeRequest(
    "/native/preferences",
    { sessionToken: "s" },
    { method: "PUT", json: { a: 1 }, fetcher },
  );
  const [, json] = fetcher.mock.calls[0]!;
  expect(json?.body).toBe('{"a":1}');
  expect(json?.headers).toMatchObject({ "Content-Type": "application/json" });

  const upload = stub();
  await nativeRequest(
    "/native/publications/import",
    { sessionToken: "s" },
    { method: "POST", body: "bytes", contentType: "application/epub+zip", fetcher: upload },
  );
  const [, binary] = upload.mock.calls[0]!;
  expect(binary?.headers).toMatchObject({ "Content-Type": "application/epub+zip" });
  expect(binary?.body).toBe("bytes");
});

test("the session token never leaves the host API", async () => {
  const fetcher = stub();
  await expect(
    nativeRequest("/THIRD_PARTY_NOTICES.md", { sessionToken: "secret" }, { fetcher }),
  ).rejects.toThrow("Refusing to send the session token to /THIRD_PARTY_NOTICES.md");
  await expect(
    nativeRequest(
      "https://example.test/native/publications",
      { sessionToken: "secret" },
      { fetcher },
    ),
  ).rejects.toThrow("Refusing to send the session token");
  expect(fetcher).not.toHaveBeenCalled();
});

test("the signal and keepalive reach the request", async () => {
  const fetcher = stub();
  const signal = new AbortController().signal;
  await nativeRequest(
    "/native/reading-activity",
    { sessionToken: "s" },
    { method: "PUT", keepalive: true, signal, fetcher },
  );
  const [, init] = fetcher.mock.calls[0]!;
  expect(init).toMatchObject({ method: "PUT", keepalive: true, signal, cache: "no-store" });
});
