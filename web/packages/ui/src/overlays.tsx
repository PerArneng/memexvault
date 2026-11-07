import * as RadixDialog from "@radix-ui/react-dialog";
import * as RadixMenu from "@radix-ui/react-dropdown-menu";
import * as RadixSelect from "@radix-ui/react-select";
import * as RadixTabs from "@radix-ui/react-tabs";
import * as RadixToast from "@radix-ui/react-toast";
import * as RadixTooltip from "@radix-ui/react-tooltip";
import { Check, ChevronDown, X } from "lucide-react";
import { useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";

const PANEL = "rounded-lg border border-border bg-surface-overlay shadow-overlay";

/** A controlled dialog has no Radix trigger to go back to, so it remembers where focus was. */
const useReturnFocus = () => {
  const opener = useRef<HTMLElement | null>(null);
  return {
    onOpenAutoFocus: () => {
      opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    },
    onCloseAutoFocus: (e: Event) => {
      if (opener.current !== null) {
        e.preventDefault();
        opener.current.focus();
      }
    },
  };
};

export interface DialogProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly title: string;
  readonly description?: string;
  readonly children?: ReactNode;
}

export const Dialog = ({ open, onOpenChange, title, description, children }: DialogProps) => {
  const returnFocus = useReturnFocus();
  return (
  <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
    <RadixDialog.Portal>
      <RadixDialog.Overlay className="fixed inset-0 bg-surface/80" />
      <RadixDialog.Content
        {...returnFocus}
        {...(description === undefined ? { "aria-describedby": undefined } : {})}
        className={`fixed top-1/2 left-1/2 w-[28rem] max-w-[90vw] -translate-x-1/2 -translate-y-1/2 p-4 ${PANEL}`}
      >
        <div className="mb-2 flex items-center justify-between">
          <RadixDialog.Title className="text-lg font-medium">{title}</RadixDialog.Title>
          <RadixDialog.Close aria-label="Close" className="rounded-sm p-1 text-text-muted hover:text-text">
            <X size={16} aria-hidden={true} />
          </RadixDialog.Close>
        </div>
        {description !== undefined && <RadixDialog.Description className="mb-3 text-sm text-text-muted">{description}</RadixDialog.Description>}
        {children}
      </RadixDialog.Content>
    </RadixDialog.Portal>
  </RadixDialog.Root>
  );
};

export interface MenuItem {
  readonly id: string;
  readonly label: string;
  readonly onSelect: () => void;
}

export const Menu = ({ trigger, items }: { readonly trigger: ReactNode; readonly items: readonly MenuItem[] }) => (
  <RadixMenu.Root modal={false}>
    <RadixMenu.Trigger asChild>{trigger}</RadixMenu.Trigger>
    <RadixMenu.Portal>
      <RadixMenu.Content sideOffset={4} className={`max-h-80 min-w-40 overflow-auto p-1 ${PANEL}`}>
        {items.map((item) => (
          <RadixMenu.Item key={item.id} onSelect={item.onSelect} className="cursor-pointer rounded-sm px-2 py-1 text-sm outline-none data-[highlighted]:bg-surface-selected">
            {item.label}
          </RadixMenu.Item>
        ))}
      </RadixMenu.Content>
    </RadixMenu.Portal>
  </RadixMenu.Root>
);

export interface SelectOption {
  readonly value: string;
  readonly label: string;
}

export const Select = ({ label, value, onValueChange, options }: { readonly label: string; readonly value: string; readonly onValueChange: (value: string) => void; readonly options: readonly SelectOption[] }) => (
  <RadixSelect.Root value={value} onValueChange={onValueChange}>
    <RadixSelect.Trigger aria-label={label} className="inline-flex h-8 items-center justify-between gap-2 rounded-md border border-border-strong bg-surface-raised px-2 text-sm">
      <RadixSelect.Value />
      <RadixSelect.Icon>
        <ChevronDown size={14} aria-hidden={true} />
      </RadixSelect.Icon>
    </RadixSelect.Trigger>
    <RadixSelect.Portal>
      <RadixSelect.Content position="popper" sideOffset={4} className={`p-1 ${PANEL}`}>
        <RadixSelect.Viewport>
          {options.map((o) => (
            <RadixSelect.Item key={o.value} value={o.value} className="flex cursor-pointer items-center justify-between gap-4 rounded-sm px-2 py-1 text-sm outline-none data-[highlighted]:bg-surface-selected">
              <RadixSelect.ItemText>{o.label}</RadixSelect.ItemText>
              <RadixSelect.ItemIndicator>
                <Check size={14} aria-hidden={true} />
              </RadixSelect.ItemIndicator>
            </RadixSelect.Item>
          ))}
        </RadixSelect.Viewport>
      </RadixSelect.Content>
    </RadixSelect.Portal>
  </RadixSelect.Root>
);

export interface TabItem {
  readonly id: string;
  readonly label: string;
  readonly content: ReactNode;
}

export const Tabs = ({ label, tabs }: { readonly label: string; readonly tabs: readonly TabItem[] }) => (
  <RadixTabs.Root defaultValue={tabs[0]?.id ?? ""}>
    <RadixTabs.List aria-label={label} className="flex gap-1 border-b border-border">
      {tabs.map((t) => (
        <RadixTabs.Trigger key={t.id} value={t.id} className="-mb-px border-b-2 border-transparent px-3 py-1.5 text-sm text-text-muted hover:text-text data-[state=active]:border-accent data-[state=active]:text-text">
          {t.label}
        </RadixTabs.Trigger>
      ))}
    </RadixTabs.List>
    {tabs.map((t) => (
      <RadixTabs.Content key={t.id} value={t.id} className="pt-3 text-sm">
        {t.content}
      </RadixTabs.Content>
    ))}
  </RadixTabs.Root>
);

export const Tooltip = ({ text, children }: { readonly text: string; readonly children: ReactNode }) => (
  <RadixTooltip.Provider delayDuration={200}>
    <RadixTooltip.Root>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content sideOffset={4} className={`px-2 py-1 text-xs ${PANEL}`}>
          {text}
        </RadixTooltip.Content>
      </RadixTooltip.Portal>
    </RadixTooltip.Root>
  </RadixTooltip.Provider>
);

export interface ToastMessage {
  readonly id: string;
  readonly title: string;
  readonly detail?: string;
}

export const Toasts = ({ messages, onDismiss }: { readonly messages: readonly ToastMessage[]; readonly onDismiss: (id: string) => void }) => (
  <RadixToast.Provider swipeDirection="right">
    {messages.map((m) => (
      <RadixToast.Root key={m.id} onOpenChange={(open) => { if (!open) onDismiss(m.id); }} className={`flex items-start justify-between gap-3 p-3 ${PANEL}`}>
        <div>
          <RadixToast.Title className="text-sm font-medium">{m.title}</RadixToast.Title>
          {m.detail !== undefined && <RadixToast.Description className="text-xs text-text-muted">{m.detail}</RadixToast.Description>}
        </div>
        <RadixToast.Close aria-label="Dismiss" className="rounded-sm p-1 text-text-muted hover:text-text">
          <X size={14} aria-hidden={true} />
        </RadixToast.Close>
      </RadixToast.Root>
    ))}
    <RadixToast.Viewport className="fixed right-4 bottom-4 z-50 flex w-80 flex-col gap-2 outline-none" />
  </RadixToast.Provider>
);

export interface Command {
  readonly id: string;
  readonly label: string;
  readonly hint?: string;
  readonly run: () => void;
}

/** Filtering is a plain substring match on the label. Arrow keys move, Enter runs. */
export const CommandPalette = ({ open, onOpenChange, commands }: { readonly open: boolean; readonly onOpenChange: (open: boolean) => void; readonly commands: readonly Command[] }) => {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const returnFocus = useReturnFocus();
  const shown = commands.filter((c) => c.label.toLowerCase().includes(query.toLowerCase()));
  const change = (next: boolean) => { setQuery(""); setActive(0); onOpenChange(next); };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") { e.preventDefault(); setActive((a) => Math.min(a + 1, shown.length - 1)); }
    if (e.key === "ArrowUp") { e.preventDefault(); setActive((a) => Math.max(a - 1, 0)); }
    if (e.key === "Enter") {
      const chosen = shown[active];
      if (chosen !== undefined) { chosen.run(); change(false); }
    }
  };
  return (
    <RadixDialog.Root open={open} onOpenChange={change}>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-0 bg-surface/80" />
        <RadixDialog.Content {...returnFocus} aria-describedby={undefined} className={`fixed top-24 left-1/2 w-[32rem] max-w-[90vw] -translate-x-1/2 ${PANEL}`}>
          <RadixDialog.Title className="sr-only">Command palette</RadixDialog.Title>
          <input
            aria-label="Search commands"
            role="combobox"
            aria-expanded={true}
            aria-controls="command-list"
            aria-activedescendant={shown[active] === undefined ? undefined : `command-${shown[active].id}`}
            value={query}
            onChange={(e) => { setQuery(e.target.value); setActive(0); }}
            onKeyDown={onKeyDown}
            placeholder="Type a command"
            className="h-10 w-full rounded-t-lg border-b border-border bg-transparent px-3 text-sm outline-none placeholder:text-text-faint"
          />
          <ul id="command-list" role="listbox" aria-label="Commands" className="max-h-72 overflow-auto p-1">
            {shown.map((c, i) => (
              <li
                key={c.id}
                id={`command-${c.id}`}
                role="option"
                aria-selected={i === active}
                onClick={() => { c.run(); change(false); }}
                className={`flex cursor-pointer justify-between rounded-sm px-2 py-1 text-sm ${i === active ? "bg-surface-selected" : ""}`}
              >
                <span>{c.label}</span>
                {c.hint !== undefined && <span className="font-mono text-xs text-text-muted">{c.hint}</span>}
              </li>
            ))}
            {shown.length === 0 && <li role="presentation" className="px-2 py-1 text-sm text-text-muted">No matching command</li>}
          </ul>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
};
