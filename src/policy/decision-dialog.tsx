import { Button } from "epub-navigator/ui";
import { AppDialog } from "../ui/dialog";
import { useAtomValue, useSetAtom } from "jotai";
import { completePolicyDecisionAtom, policyDecisionPromptAtom } from "./decision-state";

export function PolicyDecisionDialog({ container }: { readonly container: HTMLElement | null }) {
  const prompt = useAtomValue(policyDecisionPromptAtom);
  const complete = useSetAtom(completePolicyDecisionAtom);
  if (prompt === null) return null;
  return (
    <AppDialog
      key={prompt.id}
      kind="decision"
      kicker="Publication capability"
      title={prompt.title}
      description={prompt.description}
      pageLabel="Permission"
      container={container}
      onClose={() => complete(null, prompt.id)}
      actions={
        <>
          <Button variant="outline" onClick={() => complete(null, prompt.id)}>
            Cancel
          </Button>
          <Button variant="outline" onClick={() => complete(false, prompt.id)}>
            {prompt.denyLabel ?? "Keep disabled"}
          </Button>
          <Button onClick={() => complete(true, prompt.id)}>{prompt.allowLabel ?? "Allow"}</Button>
        </>
      }
    />
  );
}
