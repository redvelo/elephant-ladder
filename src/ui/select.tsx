import { Select } from "@base-ui/react/select";

export function AppSelect<Value extends string>({
  label,
  value,
  options,
  onChange,
  disabled = false,
  container,
}: {
  label: string;
  value: Value;
  options: readonly { value: Value; label: string }[];
  onChange: (value: Value) => void;
  disabled?: boolean;
  container: HTMLElement | null;
}) {
  return (
    <Select.Root
      value={value}
      items={options}
      disabled={disabled}
      onValueChange={(next) => {
        if (next !== null) onChange(next);
      }}
    >
      <Select.Trigger className="app-select-trigger" aria-label={label}>
        <Select.Value />
        <Select.Icon aria-hidden="true">⌄</Select.Icon>
      </Select.Trigger>
      <Select.Portal container={container}>
        <Select.Positioner
          className="app-select-positioner"
          sideOffset={4}
          alignItemWithTrigger={false}
        >
          <Select.Popup className="app-select-popup rounded-2xl border border-border bg-popover p-1 text-sm text-popover-foreground outline-none">
            <Select.List>
              {options.map((option) => (
                <Select.Item
                  className="app-select-item grid min-h-11 cursor-default grid-cols-[minmax(0,1fr)_1rem] items-center gap-2 rounded-xl px-2.5 text-sm outline-none select-none data-highlighted:bg-[var(--reader-selected)] data-highlighted:text-[var(--reader-selected-foreground)]"
                  key={option.value}
                  value={option.value}
                >
                  <Select.ItemText>{option.label}</Select.ItemText>
                  <Select.ItemIndicator aria-hidden="true">✓</Select.ItemIndicator>
                </Select.Item>
              ))}
            </Select.List>
          </Select.Popup>
        </Select.Positioner>
      </Select.Portal>
    </Select.Root>
  );
}
