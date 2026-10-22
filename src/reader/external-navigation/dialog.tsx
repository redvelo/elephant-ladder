import { Button } from "epub-navigator/ui";
import { AppDialog } from "../../ui/dialog";
import { useAtomValue, useSetAtom } from "jotai";
import { decideExternalNavigationAtom } from "./dispatch";
import { externalNavigationPromptAtom } from "../state";

export function ExternalNavigationDialog({
  container,
}: {
  readonly container: HTMLElement | null;
}) {
  const prompt = useAtomValue(externalNavigationPromptAtom);
  const decide = useSetAtom(decideExternalNavigationAtom);
  if (prompt === null) return null;
  const mail = prompt.kind === "mail";
  return (
    <AppDialog
      kind="decision"
      kicker={mail ? "Email address" : "External website"}
      title={mail ? "Open your email app?" : "Open in your browser?"}
      description={
        <>
          {mail ? "This link starts a message to " : "This link leaves the reader for "}
          <bdi dir="ltr">{mail ? prompt.address : prompt.origin}</bdi>.
          <span className="external-navigation-url" dir="ltr">
            {prompt.url}
          </span>
        </>
      }
      pageLabel="Link"
      container={container}
      onClose={() => decide(prompt, false)}
      actions={
        <>
          <Button variant="outline" onClick={() => decide(prompt, false)}>
            Cancel
          </Button>
          <Button onClick={() => decide(prompt, true)}>
            {mail ? "Open email app" : "Open in browser"}
          </Button>
        </>
      }
    />
  );
}
