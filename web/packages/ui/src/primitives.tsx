import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode } from "react";

type Variant = "primary" | "secondary" | "danger" | "ghost";

const VARIANTS: Readonly<Record<Variant, string>> = {
  primary: "bg-accent text-on-accent hover:bg-accent-hover font-medium",
  secondary: "bg-surface-overlay text-text border border-border-strong hover:bg-surface-selected",
  danger: "bg-danger-solid text-on-danger font-medium hover:opacity-90",
  ghost: "text-text-muted hover:text-text hover:bg-surface-overlay",
};

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  readonly variant?: Variant;
}

export const Button = ({ variant = "secondary", className = "", type = "button", ...rest }: ButtonProps) => (
  <button
    type={type}
    className={`inline-flex h-8 items-center gap-1.5 rounded-md px-3 text-sm disabled:cursor-not-allowed disabled:opacity-50 ${VARIANTS[variant]} ${className}`}
    {...rest}
  />
);

export interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  readonly label: string;
  readonly error?: string;
}

export const Input = ({ label, error, id, className = "", ...rest }: InputProps) => {
  const inputId = id ?? `input-${label.replace(/\s+/g, "-").toLowerCase()}`;
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={inputId} className="text-xs text-text-muted">
        {label}
      </label>
      <input
        id={inputId}
        aria-invalid={error !== undefined}
        {...(error !== undefined ? { "aria-describedby": `${inputId}-error` } : {})}
        className={`h-8 rounded-md border border-border-strong bg-surface-raised px-2 text-sm text-text placeholder:text-text-faint ${className}`}
        {...rest}
      />
      {error !== undefined && (
        <span id={`${inputId}-error`} className="text-xs text-danger">
          {error}
        </span>
      )}
    </div>
  );
};

export const Skeleton = ({ className = "" }: { readonly className?: string }) => (
  <div aria-hidden="true" className={`animate-pulse rounded-sm bg-neutral-surface motion-reduce:animate-none ${className}`} />
);

export const EmptyState = ({ title, children }: { readonly title: string; readonly children?: ReactNode }) => (
  <div className="flex flex-col items-center gap-2 rounded-lg border border-dashed border-border p-8 text-center">
    <p className="text-lg text-text">{title}</p>
    {children !== undefined && <div className="text-sm text-text-muted">{children}</div>}
  </div>
);

export const ErrorState = ({ title, detail, onRetry }: { readonly title: string; readonly detail?: string; readonly onRetry?: () => void }) => (
  <div role="alert" className="flex flex-col items-start gap-2 rounded-lg border border-danger bg-danger-surface p-4">
    <p className="font-medium text-danger">{title}</p>
    {detail !== undefined && <p className="font-mono text-xs text-text-muted">{detail}</p>}
    {onRetry !== undefined && <Button onClick={onRetry}>Retry</Button>}
  </div>
);

export interface TimelineItem {
  readonly id: string;
  readonly title: string;
  readonly detail?: string;
  readonly at: string;
  readonly tone?: "neutral" | "success" | "warning" | "danger" | "info";
}

const DOT: Readonly<Record<NonNullable<TimelineItem["tone"]>, string>> = {
  neutral: "bg-text-faint",
  success: "bg-success",
  warning: "bg-warning",
  danger: "bg-danger",
  info: "bg-info",
};

export const Timeline = ({ items }: { readonly items: readonly TimelineItem[] }) => (
  <ol className="flex flex-col gap-3 border-l border-border pl-4">
    {items.map((item) => (
      <li key={item.id} className="relative">
        <span aria-hidden="true" className={`absolute top-1.5 -left-[1.28rem] size-2 rounded-full ${DOT[item.tone ?? "neutral"]}`} />
        <div className="flex items-baseline justify-between gap-4">
          <span className="text-sm text-text">{item.title}</span>
          <time className="font-mono text-xs text-text-muted tabular-nums">{item.at}</time>
        </div>
        {item.detail !== undefined && <p className="text-xs text-text-muted">{item.detail}</p>}
      </li>
    ))}
  </ol>
);
