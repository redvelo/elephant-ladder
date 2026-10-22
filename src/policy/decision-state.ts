import { atom } from "jotai";
import type { PublicationCapabilityPrompt } from "./publication-policy";

export interface PolicyDecisionPrompt extends PublicationCapabilityPrompt {
  id: number;
}

interface PendingPolicyDecision {
  readonly id: number;
  readonly signal: AbortSignal;
  readonly abort: () => void;
  readonly resolve: (allowed: boolean | null) => void;
}

export const policyDecisionPromptAtom = atom<PolicyDecisionPrompt | null>(null);
const pendingPolicyDecisionAtom = atom<PendingPolicyDecision | null>(null);
const policyDecisionSequenceAtom = atom(0);

export const completePolicyDecisionAtom = atom(
  null,
  (get, set, allowed: boolean | null, promptId?: number): void => {
    const pending = get(pendingPolicyDecisionAtom);
    if (pending === null || (promptId !== undefined && pending.id !== promptId)) return;
    set(pendingPolicyDecisionAtom, null);
    pending.signal.removeEventListener("abort", pending.abort);
    if (get(policyDecisionPromptAtom)?.id === pending.id) set(policyDecisionPromptAtom, null);
    pending.resolve(allowed);
  },
);

export const requestPolicyDecisionAtom = atom(
  null,
  (
    get,
    set,
    prompt: Omit<PolicyDecisionPrompt, "id">,
    signal: AbortSignal,
  ): Promise<boolean | null> =>
    new Promise((resolve) => {
      if (signal.aborted) {
        resolve(null);
        return;
      }
      set(completePolicyDecisionAtom, null);
      const id = get(policyDecisionSequenceAtom) + 1;
      set(policyDecisionSequenceAtom, id);
      const abort = (): void => {
        if (get(pendingPolicyDecisionAtom)?.id !== id) return;
        set(pendingPolicyDecisionAtom, null);
        if (get(policyDecisionPromptAtom)?.id === id) set(policyDecisionPromptAtom, null);
        resolve(null);
      };
      set(pendingPolicyDecisionAtom, { id, signal, abort, resolve });
      signal.addEventListener("abort", abort, { once: true });
      set(policyDecisionPromptAtom, { id, ...prompt });
    }),
);
