import { Button, LampIcon } from "epub-navigator/ui";

interface FrontLightButtonProps {
  pending: boolean;
  error: string | null;
  onShow: () => void;
}

export function FrontLightButton({ pending, error, onShow }: FrontLightButtonProps) {
  return (
    <section className="front-light-dialog" aria-label="Front light">
      <Button
        className="front-light-dialog-button"
        type="button"
        variant="outline"
        size="lg"
        disabled={pending}
        onClick={onShow}
      >
        <LampIcon />
        Front light
      </Button>
      {error === null ? null : (
        <p className="front-light-error" role="alert">
          Could not open the front-light controls: {error}
        </p>
      )}
    </section>
  );
}
