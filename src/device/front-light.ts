import { atom } from "jotai";
import { authenticatedPostEmpty, describeError } from "../host/requests";
import { bootstrapAtom } from "../app/session";

export const frontLightDialogPendingAtom = atom(false);
export const frontLightDialogErrorAtom = atom<string | null>(null);

export const frontLightDialogEnabledAtom = atom(
  (get) => get(bootstrapAtom)?.nativeActions.frontLightDialog === true,
);

// The device owns the control surface and the resulting values; there is no readback.
export const showFrontLightDialogAtom = atom(null, async (get, set): Promise<void> => {
  const bootstrap = get(bootstrapAtom);
  if (bootstrap === null || get(frontLightDialogPendingAtom)) return;
  set(frontLightDialogPendingAtom, true);
  set(frontLightDialogErrorAtom, null);
  try {
    await authenticatedPostEmpty("/native/display/front-light-dialog", bootstrap.sessionToken, {});
  } catch (cause) {
    set(frontLightDialogErrorAtom, describeError(cause));
  } finally {
    set(frontLightDialogPendingAtom, false);
  }
});
