interface CatalogPackFacts {
  packId: string;
  packRevision: string;
  corpusLanguage: string;
  installedBytes: number;
  installed: boolean;
}

export interface CorpusPackSummary extends CatalogPackFacts {
  packType: "corpus";
  edition: string;
  dumpDate: string;
  monolingual: boolean;
  bilingualTargets: string[];
  licenses: string[];
}

/** Pronunciations for one language; `remote` collections play recordings from Commons. */
export interface AudioPackSummary extends CatalogPackFacts {
  packType: "audio";
  delivery: "bundled" | "remote";
  recordings: number;
}

export type CatalogPackSummary = CorpusPackSummary | AudioPackSummary;

/**
 * One installable language: its corpus and the two ways to get its pronunciations.
 *
 * Streamed pronunciations are facts rather than media and cost a fraction of the corpus,
 * so they are installed with the language rather than chosen separately. Stored
 * pronunciations replace them for offline playback.
 */
export interface DictionaryLanguage {
  corpusLanguage: string;
  corpus: CorpusPackSummary;
  streamed: AudioPackSummary | null;
  stored: AudioPackSummary | null;
  /** What the language occupies once installed the way it is offered. */
  installedBytes: number;
  /** Its corpus is on the device. */
  installed: boolean;
  /**
   * It is installed as offered: the corpus and the pronunciations that come with it.
   *
   * An install that stopped between the two leaves a language installed but incomplete,
   * which finishing the install repairs rather than a choice of delivery.
   */
  complete: boolean;
  audio: "none" | "streamed" | "stored";
}

/** The catalog as languages, skipping pronunciations with no corpus to belong to. */
export function dictionaryLanguages(packs: readonly CatalogPackSummary[]): DictionaryLanguage[] {
  const languages: DictionaryLanguage[] = [];
  for (const corpus of packs) {
    if (corpus.packType !== "corpus") continue;
    const audio = packs.filter(
      (pack): pack is AudioPackSummary =>
        pack.packType === "audio" && pack.corpusLanguage === corpus.corpusLanguage,
    );
    const streamed = audio.find((pack) => pack.delivery === "remote") ?? null;
    const stored = audio.find((pack) => pack.delivery === "bundled") ?? null;
    const installedAudio = audio.find((pack) => pack.installed);
    languages.push({
      corpusLanguage: corpus.corpusLanguage,
      corpus,
      streamed,
      stored,
      installedBytes: corpus.installedBytes + (streamed?.installedBytes ?? 0),
      installed: corpus.installed,
      complete: corpus.installed && (audio.length === 0 || installedAudio !== undefined),
      audio:
        installedAudio === undefined
          ? "none"
          : installedAudio.delivery === "bundled"
            ? "stored"
            : "streamed",
    });
  }
  return languages;
}

export interface DictionaryCatalog {
  catalogRevision: string;
  packs: CatalogPackSummary[];
}

export interface DictionaryInstallation {
  packType: "corpus" | "audio";
  packId: string;
  packRevision: string;
  corpusLanguage: string;
  installedBytes: number;
  /** How an installed pronunciation collection plays; `null` for a corpus. */
  delivery: "bundled" | "remote" | null;
}

/** One language as the device holds it, which is all there is without a catalog. */
export interface InstalledLanguage {
  corpusLanguage: string;
  installedBytes: number;
  audio: "none" | "streamed" | "stored";
}

/**
 * Installed dictionaries as languages.
 *
 * This reports what is on the device rather than what is offered, so an offline panel can
 * still list and remove languages. Pronunciations with no corpus installed are not a
 * language anyone can read.
 */
export function installedLanguages(
  installations: readonly DictionaryInstallation[],
): InstalledLanguage[] {
  const languages: InstalledLanguage[] = [];
  for (const corpus of installations) {
    if (corpus.packType !== "corpus") continue;
    const packs = installations.filter(
      (installation) => installation.corpusLanguage === corpus.corpusLanguage,
    );
    // A collection the host could not open reports no delivery, and recordings it cannot
    // open are recordings the reader does not have.
    const delivery =
      packs.find((installation) => installation.packType === "audio")?.delivery ?? null;
    languages.push({
      corpusLanguage: corpus.corpusLanguage,
      installedBytes: packs.reduce((total, installation) => total + installation.installedBytes, 0),
      audio: delivery === "bundled" ? "stored" : delivery === "remote" ? "streamed" : "none",
    });
  }
  return languages.sort((one, other) => one.corpusLanguage.localeCompare(other.corpusLanguage));
}

export type DictionaryOperationPhase =
  | "catalog"
  | "preflight"
  | "downloading"
  | "verifying"
  | "installing"
  | "completed"
  | "cancelled"
  | "failed"
  | "interrupted";

/** One language's install, which is one download however many packs it covers. */
export interface DictionaryOperation {
  operationId: string;
  corpusLanguage: string;
  audio: DictionaryAudio;
  phase: DictionaryOperationPhase;
  bytesCompleted: number;
  bytesTotal: number;
  failure: string | null;
}

/** Where a language's recordings play from, which is all an install chooses. */
export type DictionaryAudio = "streamed" | "stored";

export interface DictionaryRequestContext {
  readonly sessionToken: string;
  readonly libraryDocumentId: string;
  readonly signal?: AbortSignal;
  readonly request?: typeof fetch;
}

const terminalPhases = new Set<DictionaryOperationPhase>([
  "completed",
  "cancelled",
  "failed",
  "interrupted",
]);

export function isTerminalPhase(phase: DictionaryOperationPhase): boolean {
  return terminalPhases.has(phase);
}
