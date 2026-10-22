import {
  ACTIVE_PROBE_REPORT_TYPE,
  CONTAINER_CONSTRAINED_REPORT_TYPE,
  REMOTE_POLICY_REPORT_TYPE,
  type ProbeEvidenceValue,
  type ProbeOutcome,
  type ProbeScenario,
  type ProbeSource,
} from "./scenarios";

export type ObservationSource =
  | "EPUB/text/probe.xhtml"
  | "EPUB/text/peer.xhtml"
  | "EPUB/content.xhtml";

export interface PublicationRect {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
  readonly top: number;
  readonly right: number;
  readonly bottom: number;
  readonly left: number;
}

export interface ProbeReport {
  readonly type: typeof ACTIVE_PROBE_REPORT_TYPE;
  readonly source: ProbeSource;
  readonly scenario: ProbeScenario;
  readonly outcome: ProbeOutcome;
  readonly evidence: Readonly<Record<string, ProbeEvidenceValue | readonly ProbeEvidenceValue[]>>;
}

export interface ProbeGeneration {
  readonly generation: 1 | 2 | 3;
  readonly source: ProbeSource;
  readonly reports: ReadonlyMap<ProbeScenario, ProbeReport>;
}

export interface ContainerConstrainedReport {
  readonly type: typeof CONTAINER_CONSTRAINED_REPORT_TYPE;
  readonly source: "EPUB/content.xhtml";
  readonly childScript: boolean;
  readonly epubReadingSystem: boolean;
  readonly parentDomBlocked: boolean;
  readonly frameSizeBlocked: boolean;
  readonly readerDomBlocked: boolean;
  readonly credentialsBlocked: boolean;
  readonly nativeRouteBlocked: boolean;
}

export type RemotePolicyOutcome =
  | "blocked:connect-src"
  | "blocked:img-src"
  | "blocked:style-src"
  | "blocked:style-src-elem"
  | "blocked:script-src"
  | "blocked:script-src-elem"
  | "blocked:media-src"
  | "blocked:frame-src"
  | "loaded"
  | "failed"
  | "indeterminate";

export interface RemotePolicyReport {
  readonly type: typeof REMOTE_POLICY_REPORT_TYPE;
  readonly source: "EPUB/text/probe.xhtml";
  readonly httpsFetch: RemotePolicyOutcome;
  readonly httpsImage: RemotePolicyOutcome;
  readonly httpsStyle: RemotePolicyOutcome;
  readonly httpsScript: RemotePolicyOutcome;
  readonly httpsAudio: RemotePolicyOutcome;
  readonly httpsFrame: RemotePolicyOutcome;
  readonly wssEcho: RemotePolicyOutcome;
  readonly httpFetch: RemotePolicyOutcome;
  readonly wsEcho: RemotePolicyOutcome;
}

export type SingleReport = ContainerConstrainedReport | RemotePolicyReport;

export interface ActiveProbeContractDefinition {
  readonly workflow: "active-probe";
  readonly reportType: typeof ACTIVE_PROBE_REPORT_TYPE;
}

export interface SingleReportContractDefinition {
  readonly workflow: "single-report";
  readonly reportType: SingleReport["type"];
  readonly source: ObservationSource;
  readonly parse: (value: unknown) => SingleReport | null;
  readonly evaluate: (report: SingleReport) => RunnerCheck[];
}

export interface StaticFxlContractDefinition {
  readonly workflow: "static-fxl";
  readonly reportType: "elephant-static-fxl-parent-v1";
}

/** Records what the host and Navigator did with every document; the collector judges. */
export interface EvidenceContractDefinition {
  readonly workflow: "evidence";
  readonly reportType: "elephant-evidence-v1";
}

export type RunnerContractDefinition =
  | ActiveProbeContractDefinition
  | SingleReportContractDefinition
  | StaticFxlContractDefinition
  | EvidenceContractDefinition;

export interface StaticFxlParticipant {
  readonly occurrence: number;
  readonly resource: string;
  readonly adapter: "fixed-xhtml" | "fixed-svg";
  readonly frameGeneration: number;
  readonly layoutGeneration: number;
  readonly rect: PublicationRect;
}

export interface StaticFxlSnapshot {
  readonly kind: "elephant-static-fxl-parent-v1";
  readonly documentGeneration: number;
  readonly primary: StaticFxlParticipant;
  readonly companion:
    | (StaticFxlParticipant & {
        readonly ownerDocumentGeneration: number;
        readonly placement: "left" | "right" | "center";
      })
    | null;
  readonly visible: {
    readonly primaryOccurrence: number;
    readonly firstOccurrence: number;
    readonly lastOccurrence: number;
    readonly companion: {
      readonly occurrence: number;
      readonly placement: "left" | "right" | "center";
    } | null;
  };
  readonly hostRect: PublicationRect;
}

export type PublicationObservation =
  | {
      readonly generation: 1 | 2;
      readonly source: "EPUB/center.xhtml" | "EPUB/first.xhtml";
      readonly reports: readonly [];
      readonly snapshot: StaticFxlSnapshot;
    }
  | {
      readonly generation: 1;
      readonly source: "EPUB/content.xhtml";
      readonly reports: readonly ContainerConstrainedReport[];
    }
  | {
      readonly generation: 1 | 2;
      readonly source: "EPUB/text/probe.xhtml";
      readonly reports: readonly ProbeReport[];
    }
  | {
      readonly generation: 1;
      readonly source: "EPUB/text/probe.xhtml";
      readonly reports: readonly RemotePolicyReport[];
    }
  | {
      readonly generation: 3;
      readonly source: "EPUB/text/peer.xhtml";
      readonly reports: readonly ProbeReport[];
    };

export interface RunnerCheck {
  readonly label: string;
  readonly passed: boolean;
  readonly detail: string;
}
