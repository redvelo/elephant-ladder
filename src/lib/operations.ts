import { atom, type Getter, type PrimitiveAtom, type Setter } from "jotai";

/** One slot of work: the sequence tells a stale completion from the current one. */
export interface OperationSlot {
  readonly sequence: number;
  readonly controller: AbortController | null;
}

const IDLE_OPERATION: OperationSlot = { sequence: 0, controller: null };

export function operationAtom(): PrimitiveAtom<OperationSlot> {
  return atom(IDLE_OPERATION);
}

export function beginOperation(
  get: Getter,
  set: Setter,
  slot: PrimitiveAtom<OperationSlot>,
): OperationSlot & { readonly controller: AbortController } {
  const current = get(slot);
  current.controller?.abort();
  const next = { sequence: current.sequence + 1, controller: new AbortController() };
  set(slot, next);
  return next;
}

export function endOperation(
  get: Getter,
  set: Setter,
  slot: PrimitiveAtom<OperationSlot>,
  controller: AbortController,
): void {
  const current = get(slot);
  if (current.controller === controller)
    set(slot, { sequence: current.sequence, controller: null });
}

export function supersedeOperation(
  get: Getter,
  set: Setter,
  slot: PrimitiveAtom<OperationSlot>,
): void {
  const current = get(slot);
  current.controller?.abort();
  set(slot, { sequence: current.sequence + 1, controller: null });
}
